//! The window list on macOS: what the window server says is on screen,
//! frontmost first, already in the editor's coordinates (points from the
//! main display's top left).

use crate::windows::Window;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::CFString;
use objc2_core_graphics::{
    CGWindowListCopyWindowInfo, CGWindowListOption, kCGNullWindowID, kCGWindowAlpha, kCGWindowBounds, kCGWindowLayer, kCGWindowName,
    kCGWindowNumber, kCGWindowOwnerName, kCGWindowOwnerPID,
};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

/// Ordinary application windows; the menu bar, the Dock and floating
/// panels are on other layers.
pub fn list() -> Vec<Window> {
    let opts = CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let Some(info) = CGWindowListCopyWindowInfo(opts, kCGNullWindowID) else { return Vec::new() };
    // CFArray and CFDictionary are toll-free bridged with their NS twins.
    let info: &NSArray<NSDictionary<NSString, AnyObject>> = unsafe { &*(&*info as *const _ as *const _) };
    let me = std::process::id() as i64;
    let wins: Vec<Window> = info
        .iter()
        .filter_map(|d| {
            if number(&d, unsafe { kCGWindowLayer })? != 0.0 || number(&d, unsafe { kCGWindowAlpha }).unwrap_or(1.0) == 0.0 {
                return None;
            }
            if number(&d, unsafe { kCGWindowOwnerPID }).map(|p| p as i64) == Some(me) {
                return None;
            }
            let b = get(&d, unsafe { kCGWindowBounds })?.downcast::<NSDictionary>().ok()?;
            let b: &NSDictionary<NSString, AnyObject> = unsafe { &*(&*b as *const NSDictionary as *const _) };
            let f = |k: &str| number(b, &CFString::from_str(k));
            Some(Window {
                x: f("X")? as f32,
                y: f("Y")? as f32,
                width: f("Width")? as f32,
                height: f("Height")? as f32,
                app: string(&d, unsafe { kCGWindowOwnerName }).unwrap_or_default(),
                title: string(&d, unsafe { kCGWindowName }).unwrap_or_default(),
                id: number(&d, unsafe { kCGWindowNumber }).map(|n| (n as u64).to_string()).unwrap_or_default(),
            })
        })
        .filter(|w| w.width >= 2.0 && w.height >= 2.0)
        .collect();
    if std::env::var_os("CLIPT_DEBUG").is_some() {
        for w in &wins {
            eprintln!("clipt:   {:>6.0},{:<6.0} {:>5.0}x{:<5.0} {} — {}", w.x, w.y, w.width, w.height, w.app, w.title);
        }
    }
    wins
}

fn get(d: &NSDictionary<NSString, AnyObject>, key: &CFString) -> Option<Retained<AnyObject>> {
    // A CFString is an NSString.
    let key: &NSString = unsafe { &*(key as *const CFString as *const NSString) };
    d.objectForKey(key)
}

fn number(d: &NSDictionary<NSString, AnyObject>, key: &CFString) -> Option<f64> {
    Some(get(d, key)?.downcast::<NSNumber>().ok()?.doubleValue())
}

fn string(d: &NSDictionary<NSString, AnyObject>, key: &CFString) -> Option<String> {
    Some(get(d, key)?.downcast::<NSString>().ok()?.to_string())
}
