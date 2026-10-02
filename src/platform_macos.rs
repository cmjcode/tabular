//! Integrasi khusus macOS: Apple Event `tabular://` (M1), perintah
//! AppleScript (M4), Handoff (M5), Touch ID (M7), dan bahasa sistem (M6).
//!
//! winit memiliki `NSApplicationDelegate`, jadi fitur yang biasanya lewat
//! delegate dipasang dengan cara lain:
//! - URL dan perintah AppleScript lewat `NSAppleEventManager` (didaftarkan
//!   sebelum event loop berjalan supaya URL yang meluncurkan aplikasi tidak
//!   hilang).
//! - Handoff masuk lewat method `application:continueUserActivity:...` yang
//!   ditambahkan ke kelas delegate winit saat runtime.
//!
//! Semua data yang diterima hanya dimasukkan ke `deeplink::push_incoming`;
//! validasi dan konfirmasi terjadi di GUI.

use std::ffi::CStr;
use std::sync::mpsc;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObject, Sel};
use objc2::{AllocAnyThread, ClassType, define_class, msg_send, sel};
use objc2_foundation::{NSDictionary, NSLocale, NSString, NSUserActivity};

const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

const K_INTERNET_EVENT_CLASS: u32 = fourcc(b"GURL");
const K_AE_GET_URL: u32 = fourcc(b"GURL");
const KEY_DIRECT_OBJECT: u32 = fourcc(b"----");

/// Suite AppleScript Tabular (lihat `apple/macos/Tabular.sdef`).
const SUITE: u32 = fourcc(b"Tbul");
const EV_OPEN_URL: u32 = fourcc(b"opUR");
const EV_OPEN_CONNECTION: u32 = fourcc(b"opCN");
const EV_NEW_QUERY: u32 = fourcc(b"nwQY");
const EV_CONNECTION_NAMES: u32 = fourcc(b"lsCN");
const KEY_DATABASE: u32 = fourcc(b"kDB ");
const KEY_TABLE: u32 = fourcc(b"kTBL");
const KEY_CONNECTION: u32 = fourcc(b"kCON");

/// Tipe aktivitas Handoff; harus sama dengan `NSUserActivityTypes` di Info.plist.
pub const HANDOFF_ACTIVITY_TYPE: &str = "id.tabular.database.query";
const HANDOFF_URL_KEY: &str = "url";

define_class!(
    // SAFETY: NSObject tidak punya syarat subclass; kelas ini tanpa Drop.
    #[unsafe(super(NSObject))]
    #[name = "TabularAppleEventHandler"]
    struct AppleEventHandler;

    impl AppleEventHandler {
        #[unsafe(method(handleGetURLEvent:withReplyEvent:))]
        fn handle_get_url(&self, event: &AnyObject, _reply: &AnyObject) {
            if let Some(url) = param_string(event, KEY_DIRECT_OBJECT) {
                log::debug!("[DEEPLINK] Apple Event URL received");
                crate::deeplink::push_incoming(url);
            }
        }

        #[unsafe(method(handleScriptEvent:withReplyEvent:))]
        fn handle_script(&self, event: &AnyObject, reply: &AnyObject) {
            handle_script_event(event, reply);
        }
    }
);

impl AppleEventHandler {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

fn param_string(event: &AnyObject, keyword: u32) -> Option<String> {
    unsafe {
        let desc: Option<Retained<AnyObject>> =
            msg_send![event, paramDescriptorForKeyword: keyword];
        let desc = desc?;
        let s: Option<Retained<NSString>> = msg_send![&*desc, stringValue];
        s.map(|s| s.to_string()).filter(|s| !s.trim().is_empty())
    }
}

fn handle_script_event(event: &AnyObject, reply: &AnyObject) {
    let id: u32 = unsafe { msg_send![event, eventID] };
    match id {
        EV_OPEN_URL => {
            if let Some(url) = param_string(event, KEY_DIRECT_OBJECT) {
                crate::deeplink::push_incoming(url);
            }
        }
        EV_OPEN_CONNECTION => {
            if let Some(connection) = param_string(event, KEY_DIRECT_OBJECT) {
                let link = crate::deeplink::DeepLink::Open {
                    connection,
                    database: param_string(event, KEY_DATABASE),
                    table: param_string(event, KEY_TABLE),
                };
                crate::deeplink::push_incoming(link.to_url());
            }
        }
        EV_NEW_QUERY => {
            let sql = param_string(event, KEY_DIRECT_OBJECT);
            let connection = param_string(event, KEY_CONNECTION);
            if let (Some(sql), Some(connection)) = (sql, connection) {
                let link = crate::deeplink::DeepLink::Query {
                    connection,
                    sql,
                    database: param_string(event, KEY_DATABASE),
                    run: false,
                };
                crate::deeplink::push_incoming(link.to_url());
            }
        }
        EV_CONNECTION_NAMES => unsafe {
            let Some(list_cls) = AnyClass::get(c"NSAppleEventDescriptor") else {
                return;
            };
            let list: Retained<AnyObject> = msg_send![list_cls, listDescriptor];
            for (i, (_, name)) in crate::deeplink::known_connections().iter().enumerate() {
                let ns = NSString::from_str(name);
                let item: Retained<AnyObject> = msg_send![list_cls, descriptorWithString: &*ns];
                let _: () = msg_send![&*list, insertDescriptor: &*item, atIndex: (i as isize) + 1];
            }
            let _: () = msg_send![reply, setParamDescriptor: &*list, forKeyword: KEY_DIRECT_OBJECT];
        },
        other => log::debug!("[APPLESCRIPT] unknown event id {other:#x}"),
    }
}

/// Daftarkan handler Apple Event. Panggil sekali, sebelum `eframe::run_native`.
pub fn install_apple_event_handlers() {
    let Some(manager_cls) = AnyClass::get(c"NSAppleEventManager") else {
        log::warn!("[DEEPLINK] NSAppleEventManager unavailable");
        return;
    };
    let handler = AppleEventHandler::new();
    unsafe {
        let manager: Retained<AnyObject> = msg_send![manager_cls, sharedAppleEventManager];
        let _: () = msg_send![
            &*manager,
            setEventHandler: &*handler,
            andSelector: sel!(handleGetURLEvent:withReplyEvent:),
            forEventClass: K_INTERNET_EVENT_CLASS,
            andEventID: K_AE_GET_URL
        ];
        for id in [
            EV_OPEN_URL,
            EV_OPEN_CONNECTION,
            EV_NEW_QUERY,
            EV_CONNECTION_NAMES,
        ] {
            let _: () = msg_send![
                &*manager,
                setEventHandler: &*handler,
                andSelector: sel!(handleScriptEvent:withReplyEvent:),
                forEventClass: SUITE,
                andEventID: id
            ];
        }
    }
    // Event manager tidak me-retain handler; hidup sepanjang proses.
    std::mem::forget(handler);
}

// ─────────────────────────────────────────────────────────────────────────────
// Handoff
// ─────────────────────────────────────────────────────────────────────────────

thread_local! {
    static CURRENT_ACTIVITY: std::cell::RefCell<Option<(String, Retained<NSUserActivity>)>> =
        const { std::cell::RefCell::new(None) };
}

/// Umumkan (atau hapus bila `None`) aktivitas Handoff `(judul, url)`.
/// Dipanggil dari thread UI; tidak berbuat apa-apa bila url tidak berubah.
pub fn set_handoff_activity(activity: Option<(&str, &str)>) {
    CURRENT_ACTIVITY.with(|cell| {
        let mut cur = cell.borrow_mut();
        let same = match (&*cur, activity) {
            (Some((u, _)), Some((_, url))) => u == url,
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        if let Some((_, old)) = cur.take() {
            old.invalidate();
        }
        let Some((title, url)) = activity else {
            return;
        };
        let act = NSUserActivity::initWithActivityType(
            NSUserActivity::alloc(),
            &NSString::from_str(HANDOFF_ACTIVITY_TYPE),
        );
        act.setTitle(Some(&NSString::from_str(title)));
        let key = NSString::from_str(HANDOFF_URL_KEY);
        let value = NSString::from_str(url);
        let dict: Retained<NSDictionary<NSString, NSString>> =
            NSDictionary::from_slices(&[&*key], &[&*value]);
        // SAFETY: userInfo hanya berisi NSString (tipe plist yang valid).
        unsafe {
            let dict: Retained<NSDictionary> = Retained::cast_unchecked(dict);
            act.setUserInfo(Some(&dict));
        }
        act.setEligibleForHandoff(true);
        act.becomeCurrent();
        *cur = Some((url.to_string(), act));
    });
}

type ContinueImp = extern "C-unwind" fn(
    *mut AnyObject,
    Sel,
    *mut AnyObject,
    *mut AnyObject,
    *mut AnyObject,
) -> Bool;

extern "C-unwind" fn continue_user_activity(
    _this: *mut AnyObject,
    _cmd: Sel,
    _app: *mut AnyObject,
    activity: *mut AnyObject,
    _restoration: *mut AnyObject,
) -> Bool {
    let Some(activity) = (unsafe { activity.as_ref() }) else {
        return Bool::NO;
    };
    unsafe {
        let kind: Option<Retained<NSString>> = msg_send![activity, activityType];
        if kind.map(|k| k.to_string()).as_deref() != Some(HANDOFF_ACTIVITY_TYPE) {
            return Bool::NO;
        }
        let info: Option<Retained<AnyObject>> = msg_send![activity, userInfo];
        let Some(info) = info else {
            return Bool::NO;
        };
        let key = NSString::from_str(HANDOFF_URL_KEY);
        let value: Option<Retained<AnyObject>> = msg_send![&*info, objectForKey: &*key];
        let Some(value) = value else {
            return Bool::NO;
        };
        let is_string: bool = msg_send![&*value, isKindOfClass: NSString::class()];
        if !is_string {
            return Bool::NO;
        }
        let url: Retained<NSString> = Retained::cast_unchecked(value);
        log::debug!("[HANDOFF] continuing activity");
        crate::deeplink::push_incoming(url.to_string());
    }
    Bool::YES
}

/// Tambahkan `application:continueUserActivity:restorationHandler:` ke kelas
/// delegate aplikasi (milik winit). Panggil dari thread UI setelah jendela
/// dibuat; aman dipanggil berulang.
pub fn install_handoff_receiver() {
    unsafe {
        let Some(app_cls) = AnyClass::get(c"NSApplication") else {
            return;
        };
        let app: Option<Retained<AnyObject>> = msg_send![app_cls, sharedApplication];
        let Some(app) = app else {
            return;
        };
        let delegate: Option<Retained<AnyObject>> = msg_send![&*app, delegate];
        let Some(delegate) = delegate else {
            log::debug!("[HANDOFF] no app delegate yet");
            return;
        };
        let sel = sel!(application:continueUserActivity:restorationHandler:);
        let cls = delegate.class();
        if cls.responds_to(sel) {
            return;
        }
        let imp: objc2::runtime::Imp =
            std::mem::transmute::<ContinueImp, objc2::runtime::Imp>(continue_user_activity);
        let types: &CStr = c"B@:@@@?";
        let added = objc2::ffi::class_addMethod(
            cls as *const AnyClass as *mut AnyClass,
            sel,
            imp,
            types.as_ptr(),
        );
        if !added.as_bool() {
            log::warn!("[HANDOFF] could not install continueUserActivity handler");
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Touch ID (LocalAuthentication)
// ─────────────────────────────────────────────────────────────────────────────

#[link(name = "LocalAuthentication", kind = "framework")]
unsafe extern "C" {}

/// LAPolicyDeviceOwnerAuthenticationWithBiometrics.
const LA_POLICY_BIOMETRICS: isize = 1;

fn new_la_context() -> Option<Retained<AnyObject>> {
    let cls = AnyClass::get(c"LAContext")?;
    unsafe { msg_send![cls, new] }
}

/// Apakah Touch ID tersedia dan sudah didaftarkan di Mac ini.
pub fn biometric_available() -> bool {
    let Some(ctx) = new_la_context() else {
        return false;
    };
    unsafe {
        msg_send![
            &*ctx,
            canEvaluatePolicy: LA_POLICY_BIOMETRICS,
            error: std::ptr::null_mut::<*mut AnyObject>()
        ]
    }
}

/// Minta Touch ID. Hasil dikirim ke channel dari thread LocalAuthentication:
/// `Ok(())` bila berhasil, `Err(pesan)` bila gagal atau dibatalkan.
pub fn authenticate(reason: &str) -> mpsc::Receiver<Result<(), String>> {
    let (tx, rx) = mpsc::channel();
    let Some(ctx) = new_la_context() else {
        let _ = tx.send(Err("Touch ID is not available on this Mac".into()));
        return rx;
    };
    let tx = std::sync::Mutex::new(Some(tx));
    let block = block2::RcBlock::new(move |success: Bool, _error: *mut AnyObject| {
        if let Some(tx) = tx.lock().ok().and_then(|mut g| g.take()) {
            let _ = tx.send(if success.as_bool() {
                Ok(())
            } else {
                Err("Touch ID was cancelled or did not match".into())
            });
        }
    });
    let reason = NSString::from_str(reason);
    unsafe {
        let _: () = msg_send![
            &*ctx,
            evaluatePolicy: LA_POLICY_BIOMETRICS,
            localizedReason: &*reason,
            reply: &*block
        ];
    }
    rx
}

// ─────────────────────────────────────────────────────────────────────────────
// Bahasa
// ─────────────────────────────────────────────────────────────────────────────

/// Bahasa pertama di System Settings > Language & Region, mis. `id-ID`.
pub fn preferred_language() -> Option<String> {
    NSLocale::preferredLanguages()
        .firstObject()
        .map(|s| s.to_string())
}
