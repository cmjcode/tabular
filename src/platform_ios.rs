//! Integrasi iOS/iPadOS (M1, M5): menerima `tabular://` (termasuk dari aksi
//! Shortcuts "Open URL") dan Handoff dari Mac.
//!
//! `UIApplicationDelegate` dimiliki winit, jadi method delegate yang dibutuhkan
//! ditambahkan ke kelasnya saat runtime. [`install_url_receivers`] dipanggil
//! dari closure pembuatan aplikasi eframe, yang di iOS berjalan di dalam
//! `didFinishLaunching`, sehingga URL yang meluncurkan aplikasi tetap diterima.

use std::ffi::CStr;

use objc2::msg_send;
use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
use objc2::sel;
use objc2_foundation::NSString;

const HANDOFF_ACTIVITY_TYPE: &str = "id.tabular.database.query";

unsafe fn ns_to_string(obj: *mut AnyObject) -> Option<String> {
    let s = obj as *const NSString;
    unsafe { s.as_ref() }.map(|s| s.to_string())
}

extern "C" fn open_url(
    _this: *mut AnyObject,
    _cmd: Sel,
    _app: *mut AnyObject,
    url: *mut AnyObject,
    _options: *mut AnyObject,
) -> Bool {
    if url.is_null() {
        return Bool::NO;
    }
    let text = unsafe {
        let s: *mut AnyObject = msg_send![url, absoluteString];
        ns_to_string(s)
    };
    match text {
        Some(t) if t.to_ascii_lowercase().starts_with("tabular:") => {
            crate::deeplink::push_incoming(t);
            Bool::YES
        }
        _ => Bool::NO,
    }
}

extern "C" fn continue_user_activity(
    _this: *mut AnyObject,
    _cmd: Sel,
    _app: *mut AnyObject,
    activity: *mut AnyObject,
    _restoration: *mut AnyObject,
) -> Bool {
    if activity.is_null() {
        return Bool::NO;
    }
    unsafe {
        let kind: *mut AnyObject = msg_send![activity, activityType];
        if ns_to_string(kind).as_deref() != Some(HANDOFF_ACTIVITY_TYPE) {
            return Bool::NO;
        }
        let info: *mut AnyObject = msg_send![activity, userInfo];
        if info.is_null() {
            return Bool::NO;
        }
        let key = NSString::from_str("url");
        let value: *mut AnyObject = msg_send![info, objectForKey: &*key];
        let Some(url) = ns_to_string(value) else {
            return Bool::NO;
        };
        crate::deeplink::push_incoming(url);
    }
    Bool::YES
}

unsafe fn add_method(cls: &AnyClass, sel: Sel, imp: objc2::runtime::Imp, types: &CStr) {
    if cls.responds_to(sel) {
        return;
    }
    let added = unsafe {
        objc2::ffi::class_addMethod(
            cls as *const AnyClass as *mut objc2::ffi::objc_class,
            sel.as_ptr(),
            Some(imp),
            types.as_ptr(),
        )
    };
    if added == objc2::ffi::NO {
        log::warn!("[DEEPLINK] could not add {sel:?} to the app delegate");
    }
}

type DelegateImp =
    extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject, *mut AnyObject) -> Bool;

/// Tambahkan handler URL dan Handoff ke delegate aplikasi (milik winit).
pub fn install_url_receivers() {
    unsafe {
        let Some(app_cls) = AnyClass::get("UIApplication") else {
            return;
        };
        let app: *mut AnyObject = msg_send![app_cls, sharedApplication];
        if app.is_null() {
            return;
        }
        let delegate: *mut AnyObject = msg_send![app, delegate];
        let Some(delegate) = delegate.as_ref() else {
            log::debug!("[DEEPLINK] no UIApplication delegate yet");
            return;
        };
        let cls = delegate.class();
        add_method(
            cls,
            sel!(application:openURL:options:),
            std::mem::transmute::<DelegateImp, objc2::runtime::Imp>(open_url),
            c"B@:@@@",
        );
        add_method(
            cls,
            sel!(application:continueUserActivity:restorationHandler:),
            std::mem::transmute::<DelegateImp, objc2::runtime::Imp>(continue_user_activity),
            c"B@:@@@?",
        );
    }
}
