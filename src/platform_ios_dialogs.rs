//! Dialog berkas native iPadOS (pengganti `rfd`, yang tidak punya backend iOS).
//!
//! `rfd::FileDialog` dipakai di ~20 tempat dengan API sinkron
//! (`pick_file() -> Option<PathBuf>`), jadi modul ini meniru kontrak itu di atas
//! `UIDocumentPickerViewController` yang asinkron:
//!
//! * `pick_file` / `pick_files` / `pick_folder` menampilkan picker secara modal,
//!   lalu **memblokir** pemanggil sampai delegate dipanggil. Bila pemanggil ada di
//!   main thread, run loop utama dijalankan bersarang (pola yang sama dengan modal
//!   `NSOpenPanel` di macOS); bila dari thread lain, cukup menunggu channel.
//! * `save_file` tidak memakai picker: pemanggil menulis ke path yang dikembalikan
//!   *setelah* fungsi ini selesai, sedangkan picker ekspor iOS butuh berkasnya
//!   sudah ada. Jadi kita kembalikan path di `Documents/` aplikasi — folder itu
//!   terlihat di Files ("On My iPad > Tabular") berkat `UIFileSharingEnabled` +
//!   `LSSupportsOpeningDocumentsInPlace` di `apple/ios/Info.plist`.
//! * Berkas yang dipilih diminta `asCopy: true`, sehingga UIKit menyalinnya ke
//!   sandbox dan kita tidak perlu mengelola security-scoped bookmark. Folder tidak
//!   bisa disalin, jadi akses security-scoped dibuka dan sengaja tidak ditutup
//!   selama proses hidup (pemanggil menyimpan path, bukan URL).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{ClassType, DeclaredClass, declare_class, msg_send_id, mutability};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSDate, NSDefaultRunLoopMode, NSRunLoop, NSString, NSURL,
    run_on_main,
};
use objc2_ui_kit::{
    UIAlertAction, UIAlertActionStyle, UIAlertController, UIAlertControllerStyle, UIApplication,
    UIDocumentPickerDelegate, UIDocumentPickerViewController, UIViewController,
};
use objc2_uniform_type_identifiers::{UTType, UTTypeFolder, UTTypeItem};

/// Hasil satu sesi picker: daftar path, atau kosong bila dibatalkan.
type PickResult = Vec<PathBuf>;

struct DelegateIvars {
    tx: RefCell<Option<Sender<PickResult>>>,
}

declare_class!(
    struct PickerDelegate;

    unsafe impl ClassType for PickerDelegate {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "TabularDocumentPickerDelegate";
    }

    impl DeclaredClass for PickerDelegate {
        type Ivars = DelegateIvars;
    }

    unsafe impl NSObjectProtocol for PickerDelegate {}

    unsafe impl UIDocumentPickerDelegate for PickerDelegate {
        #[method(documentPicker:didPickDocumentsAtURLs:)]
        unsafe fn did_pick(
            &self,
            _picker: &UIDocumentPickerViewController,
            urls: &NSArray<NSURL>,
        ) {
            let paths: Vec<PathBuf> = urls
                .iter()
                .filter_map(|url| unsafe {
                    // Folder dipilih tanpa `asCopy`, jadi butuh akses security-scoped.
                    // Sengaja tidak di-stop: pemanggil hanya menyimpan `PathBuf`.
                    let _ = url.startAccessingSecurityScopedResource();
                    url.path().map(|p| PathBuf::from(p.to_string()))
                })
                .collect();
            self.finish(paths);
        }

        #[method(documentPickerWasCancelled:)]
        unsafe fn was_cancelled(&self, _picker: &UIDocumentPickerViewController) {
            self.finish(Vec::new());
        }
    }
);

impl PickerDelegate {
    fn new(mtm: MainThreadMarker, tx: Sender<PickResult>) -> Retained<Self> {
        let this = mtm.alloc().set_ivars(DelegateIvars {
            tx: RefCell::new(Some(tx)),
        });
        unsafe { msg_send_id![super(this), init] }
    }

    fn finish(&self, paths: PickResult) {
        if let Some(tx) = self.ivars().tx.borrow_mut().take() {
            let _ = tx.send(paths);
        }
        ACTIVE_DELEGATE.with(|slot| slot.borrow_mut().take());
    }
}

thread_local! {
    /// `UIDocumentPickerViewController.delegate` bersifat weak; delegate harus
    /// kita pegang sendiri sampai picker selesai. Hanya disentuh di main thread.
    static ACTIVE_DELEGATE: RefCell<Option<Retained<PickerDelegate>>> = const { RefCell::new(None) };
}

/// Jenis picker yang diminta pemanggil.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PickKind {
    File { multiple: bool },
    Folder,
}

/// View controller paling depan untuk mem-present modal.
///
/// `UIApplication.windows` ditandai deprecated sejak iOS 15 demi `UIWindowScene`,
/// tetapi masih berfungsi dan Tabular hanya punya satu scene (winit); downcast
/// `UIScene -> UIWindowScene` belum tersedia rapi di objc2 0.5.
#[allow(deprecated)]
fn presenting_controller(mtm: MainThreadMarker) -> Option<Retained<UIViewController>> {
    let app = UIApplication::sharedApplication(mtm);
    let windows = app.windows();
    let window = windows
        .iter()
        .find(|w| w.isKeyWindow())
        .or_else(|| windows.iter().next())?;
    let mut vc = window.rootViewController()?;
    while let Some(presented) = unsafe { vc.presentedViewController() } {
        vc = presented;
    }
    Some(vc)
}

/// Terjemahkan filter ekstensi `rfd` menjadi `UTType`. Ekstensi yang tidak dikenal
/// sistem diabaikan; tanpa hasil, semua berkas diizinkan.
fn content_types(extensions: &[String]) -> Retained<NSArray<UTType>> {
    let mut types: Vec<Retained<UTType>> = extensions
        .iter()
        .filter_map(|ext| {
            let ext = ext.trim_start_matches('.');
            if ext.is_empty() || ext == "*" {
                return None;
            }
            unsafe { UTType::typeWithFilenameExtension(&NSString::from_str(ext)) }
        })
        .collect();
    if types.is_empty() {
        types.push(unsafe { UTTypeItem }.retain());
    }
    NSArray::from_vec(types)
}

fn present_picker(kind: PickKind, extensions: &[String]) -> Option<PickResult> {
    let (tx, rx) = mpsc::channel::<PickResult>();
    let extensions = extensions.to_vec();

    let presented = run_on_main(move |mtm| -> bool {
        if ACTIVE_DELEGATE.with(|slot| slot.borrow().is_some()) {
            log::warn!("[IOS] document picker already open; ignoring nested request");
            return false;
        }
        let Some(presenter) = presenting_controller(mtm) else {
            log::warn!("[IOS] no root view controller to present the document picker");
            return false;
        };

        let picker = unsafe {
            match kind {
                PickKind::File { .. } => {
                    UIDocumentPickerViewController::initForOpeningContentTypes_asCopy(
                        mtm.alloc(),
                        &content_types(&extensions),
                        true,
                    )
                }
                PickKind::Folder => UIDocumentPickerViewController::initForOpeningContentTypes(
                    mtm.alloc(),
                    &NSArray::from_slice(&[UTTypeFolder]),
                ),
            }
        };
        let delegate = PickerDelegate::new(mtm, tx);
        unsafe {
            picker.setAllowsMultipleSelection(matches!(kind, PickKind::File { multiple: true }));
            picker.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            presenter.presentViewController_animated_completion(&picker, true, None);
        }
        ACTIVE_DELEGATE.with(|slot| *slot.borrow_mut() = Some(delegate));
        true
    });
    if !presented {
        return None;
    }

    wait_for_result(rx)
}

/// Tunggu delegate. Di main thread, run loop harus terus berjalan agar picker
/// bisa menerima sentuhan; di thread lain cukup `recv`.
fn wait_for_result(rx: Receiver<PickResult>) -> Option<PickResult> {
    if MainThreadMarker::new().is_some() {
        let run_loop = unsafe { NSRunLoop::currentRunLoop() };
        loop {
            match rx.try_recv() {
                Ok(paths) => return Some(paths),
                Err(mpsc::TryRecvError::Disconnected) => return None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
            let until = unsafe { NSDate::dateWithTimeIntervalSinceNow(0.05) };
            if !unsafe { run_loop.runMode_beforeDate(NSDefaultRunLoopMode, &until) } {
                // Run loop tidak punya sumber input; jangan sibuk-putar.
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    } else {
        rx.recv().ok()
    }
}

/// `Documents/` di dalam sandbox aplikasi (terlihat di Files).
fn documents_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dir = PathBuf::from(home).join("Documents");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Path unik `Documents/<name>`; menambah ` (n)` bila sudah ada.
fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    let candidate = dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("export");
    let ext = path.extension().and_then(|e| e.to_str());
    (2..)
        .map(|n| match ext {
            Some(ext) => dir.join(format!("{stem} ({n}).{ext}")),
            None => dir.join(format!("{stem} ({n})")),
        })
        .find(|p| !p.exists())
        .expect("iterator tak terbatas selalu menghasilkan nilai")
}

/// Pengganti `rfd::FileDialog` untuk iOS. Hanya `add_filter` dan `set_file_name`
/// yang berpengaruh; `set_directory`/`set_title` diterima demi kompatibilitas API.
#[derive(Default, Clone)]
pub struct FileDialog {
    extensions: Vec<String>,
    file_name: Option<String>,
}

impl FileDialog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_filter<S: AsRef<str>, T: AsRef<str>>(mut self, _name: S, ext: &[T]) -> Self {
        self.extensions
            .extend(ext.iter().map(|e| e.as_ref().to_string()));
        self
    }

    pub fn set_file_name<S: AsRef<str>>(mut self, name: S) -> Self {
        self.file_name = Some(name.as_ref().to_string());
        self
    }

    pub fn set_title<S: AsRef<str>>(self, _title: S) -> Self {
        self
    }

    pub fn set_directory<P: AsRef<Path>>(self, _dir: P) -> Self {
        self
    }

    pub fn pick_file(self) -> Option<PathBuf> {
        present_picker(PickKind::File { multiple: false }, &self.extensions)?
            .into_iter()
            .next()
    }

    pub fn pick_files(self) -> Option<Vec<PathBuf>> {
        present_picker(PickKind::File { multiple: true }, &self.extensions)
            .filter(|paths| !paths.is_empty())
    }

    pub fn pick_folder(self) -> Option<PathBuf> {
        present_picker(PickKind::Folder, &[])?.into_iter().next()
    }

    /// Lihat catatan modul: mengembalikan path tulis di `Documents/`, bukan hasil
    /// picker. Pemanggil tetap bertanggung jawab menulis berkasnya.
    pub fn save_file(self) -> Option<PathBuf> {
        let dir = documents_dir()?;
        let mut name = self
            .file_name
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "export".to_string());
        if Path::new(&name).extension().is_none()
            && let Some(ext) = self.extensions.first()
        {
            name.push('.');
            name.push_str(ext.trim_start_matches('.'));
        }
        let path = unique_path(&dir, &name);
        log::info!("[IOS] save_file resolved to {}", path.display());
        Some(path)
    }
}

/// Pengganti `rfd::MessageDialog`: `UIAlertController` dengan satu tombol OK,
/// tidak memblokir.
#[derive(Default, Clone)]
pub struct MessageDialog {
    title: String,
    description: String,
}

impl MessageDialog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_title<S: AsRef<str>>(mut self, title: S) -> Self {
        self.title = title.as_ref().to_string();
        self
    }

    pub fn set_description<S: AsRef<str>>(mut self, desc: S) -> Self {
        self.description = desc.as_ref().to_string();
        self
    }

    pub fn show(self) {
        run_on_main(move |mtm| {
            let Some(presenter) = presenting_controller(mtm) else {
                log::warn!("[IOS] no view controller to present alert: {}", self.title);
                return;
            };
            unsafe {
                let alert = UIAlertController::alertControllerWithTitle_message_preferredStyle(
                    Some(&NSString::from_str(&self.title)),
                    Some(&NSString::from_str(&self.description)),
                    UIAlertControllerStyle::Alert,
                    mtm,
                );
                let ok = UIAlertAction::actionWithTitle_style_handler(
                    Some(&NSString::from_str("OK")),
                    UIAlertActionStyle::Default,
                    None,
                    mtm,
                );
                alert.addAction(&ok);
                presenter.presentViewController_animated_completion(&alert, true, None);
            }
        });
    }
}
