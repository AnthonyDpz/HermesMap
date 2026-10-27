//! Mac : quand Hermes Map est lancé comme une app (.app), on fait tourner la boucle d'événements AppKit
//! pour répondre à « reopen » (Spotlight, Dock, Finder alors que l'app tourne déjà) en rouvrant la page.
//! AppKit et le runtime Objective-C sont chargés à l'exécution : aucune dépendance de compilation.

#![allow(non_camel_case_types)]
use std::ffi::{c_char, c_void, CString};
use std::sync::OnceLock;

type id = *mut c_void;
type SEL = *mut c_void;
type Class = *mut c_void;

extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}
const RTLD_NOW: i32 = 2;

static ON_REOPEN: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

extern "C" fn reopen(_this: id, _cmd: SEL, _sender: id, _flag: bool) -> bool {
    if let Some(f) = ON_REOPEN.get() {
        f();
    }
    true
}

unsafe fn sym(h: *mut c_void, name: &str) -> *mut c_void {
    let c = CString::new(name).unwrap();
    dlsym(h, c.as_ptr())
}

/// Lancé depuis un paquet .app ?
pub fn in_bundle() -> bool {
    std::env::current_exe().map(|p| p.to_string_lossy().contains(".app/Contents/MacOS/")).unwrap_or(false)
}

/// Fait tourner la boucle AppKit (ne revient pas). Renvoie false si AppKit n'est pas disponible.
pub fn run_app(on_reopen: Box<dyn Fn() + Send + Sync>) -> bool {
    let _ = ON_REOPEN.set(on_reopen);
    unsafe {
        let objc = dlopen(c"/usr/lib/libobjc.A.dylib".as_ptr(), RTLD_NOW);
        let appkit = dlopen(c"/System/Library/Frameworks/AppKit.framework/AppKit".as_ptr(), RTLD_NOW);
        if objc.is_null() || appkit.is_null() {
            return false;
        }
        let get_class: extern "C" fn(*const c_char) -> Class = std::mem::transmute(sym(objc, "objc_getClass"));
        let alloc_pair: extern "C" fn(Class, *const c_char, usize) -> Class = std::mem::transmute(sym(objc, "objc_allocateClassPair"));
        let reg_pair: extern "C" fn(Class) = std::mem::transmute(sym(objc, "objc_registerClassPair"));
        let add_method: extern "C" fn(Class, SEL, *const c_void, *const c_char) -> bool = std::mem::transmute(sym(objc, "class_addMethod"));
        let sel: extern "C" fn(*const c_char) -> SEL = std::mem::transmute(sym(objc, "sel_registerName"));
        let send = sym(objc, "objc_msgSend");
        if send.is_null() {
            return false;
        }
        let send0: extern "C" fn(id, SEL) -> id = std::mem::transmute(send);
        let send1: extern "C" fn(id, SEL, id) -> id = std::mem::transmute(send);
        let s = |n: &str| sel(CString::new(n).unwrap().as_ptr());
        let nsapp = get_class(c"NSApplication".as_ptr());
        let nsobject = get_class(c"NSObject".as_ptr());
        if nsapp.is_null() || nsobject.is_null() {
            return false;
        }
        let app = send0(nsapp, s("sharedApplication"));
        let dcls = alloc_pair(nsobject, c"HMDelegate".as_ptr(), 0);
        if app.is_null() || dcls.is_null() {
            return false;
        }
        add_method(dcls, s("applicationShouldHandleReopen:hasVisibleWindows:"), reopen as *const c_void, c"c@:@c".as_ptr());
        reg_pair(dcls);
        let delegate = send0(send0(dcls, s("alloc")), s("init"));
        send1(app, s("setDelegate:"), delegate);
        send0(app, s("run")); // ne revient pas
    }
    true
}
