//! Mission Control open/closed, from the Dock's own AX notifications.
//!
//! The active Space (CGSGetActiveSpace) only changes at the END of a Space
//! transition (measured on real swipes). A Mission Control click onto
//! Noki's Desktop would therefore show the Miniatur there - the same
//! Desktop inside itself - until the switch had finished. The Dock
//! announces Mission Control before any of that (`AXExposeShowAllWindows`
//! and friends, `AXExposeExit` when it closes). The helper keeps the
//! Miniatur TEMPORARILY hidden while Mission Control is open and decides
//! on the landing Space afterwards; target and presentation stay as they are.
//! Event-driven: no polling except a cheap Dock-restart check every 5 s.

#![cfg(target_os = "macos")]

use std::ffi::c_void;

type Observer = *mut c_void;
type Callback = unsafe extern "C" fn(Observer, *mut c_void, *const c_void, *mut c_void);

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXObserverCreate(pid: i32, cb: Callback, out: *mut Observer) -> i32;
    fn AXObserverAddNotification(o: Observer, el: *mut c_void, n: *const c_void, refcon: *mut c_void) -> i32;
    fn AXObserverGetRunLoopSource(o: Observer) -> *mut c_void;
    fn AXUIElementCreateApplication(pid: i32) -> *mut c_void;
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopDefaultMode: *const c_void;
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopAddSource(rl: *mut c_void, src: *mut c_void, mode: *const c_void);
    fn CFRunLoopRemoveSource(rl: *mut c_void, src: *mut c_void, mode: *const c_void);
    fn CFRunLoopRunInMode(mode: *const c_void, sec: f64, ret: bool) -> i32;
    fn CFStringCreateWithCString(a: *const c_void, s: *const i8, enc: u32) -> *const c_void;
    fn CFStringGetCString(s: *const c_void, buf: *mut i8, n: isize, enc: u32) -> bool;
    fn CFRelease(o: *const c_void);
}

const OEFFNEN: [&str; 3] = ["AXExposeShowAllWindows", "AXExposeShowFrontWindows", "AXExposeShowDesktop"];
const SCHLIESSEN: &str = "AXExposeExit";

unsafe fn cfstr(s: &str) -> *const c_void {
    let c = std::ffi::CString::new(s).unwrap_or_default();
    CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), 0x0800_0100)
}

unsafe extern "C" fn meldung(_o: Observer, _el: *mut c_void, name: *const c_void, _r: *mut c_void) {
    let mut buf = [0i8; 64];
    if !CFStringGetCString(name, buf.as_mut_ptr(), 64, 0x0800_0100) {
        return;
    }
    let n = std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy();
    let offen = OEFFNEN.contains(&n.as_ref());
    if !offen && n != SCHLIESSEN {
        return;
    }
    crate::vorschau::befehl(if offen { "mc 1" } else { "mc 0" });
    if !offen {
        if let Some(app) = crate::WACHE_APP.get() {
            let a = app.clone();
            let _ = app.run_on_main_thread(move || crate::window_overview::nach_mission_control(&a));
        }
    }
    crate::virtual_workspace::trace(&format!("[MISSION_CONTROL] {n} -> {}", if offen { "offen" } else { "zu" }));
}

pub fn starten() {
    static FADEN: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    FADEN.get_or_init(|| {
        let _ = std::thread::Builder::new().name("noki-missioncontrol".into()).spawn(|| unsafe {
            let mut dock = 0i32;
            let mut quelle: *mut c_void = std::ptr::null_mut();
            loop {
                let pid = crate::lesezeichen::pid_fuer_bundle("com.apple.dock").unwrap_or(0);
                if pid != 0 && pid != dock {
                    if !quelle.is_null() {
                        CFRunLoopRemoveSource(CFRunLoopGetCurrent(), quelle, kCFRunLoopDefaultMode);
                        quelle = std::ptr::null_mut();
                    }
                    let mut o: Observer = std::ptr::null_mut();
                    if AXObserverCreate(pid, meldung, &mut o) == 0 && !o.is_null() {
                        let app = AXUIElementCreateApplication(pid);
                        let mut ok = 0;
                        for n in OEFFNEN.iter().chain(std::iter::once(&SCHLIESSEN)) {
                            let s = cfstr(n);
                            if AXObserverAddNotification(o, app, s, std::ptr::null_mut()) == 0 {
                                ok += 1;
                            }
                            CFRelease(s);
                        }
                        quelle = AXObserverGetRunLoopSource(o);
                        CFRunLoopAddSource(CFRunLoopGetCurrent(), quelle, kCFRunLoopDefaultMode);
                        crate::virtual_workspace::trace(&format!("[MISSION_CONTROL] observer dock_pid={pid} notifications={ok}/4"));
                    }
                    dock = pid;
                }
                // 1 = kCFRunLoopRunFinished: no source (observer not created) -
                // the call returns at once; pause instead of spinning.
                if CFRunLoopRunInMode(kCFRunLoopDefaultMode, 5.0, false) == 1 {
                    std::thread::sleep(std::time::Duration::from_millis(1000));
                }
            }
        });
    });
}
