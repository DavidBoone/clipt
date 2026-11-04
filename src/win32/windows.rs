//! The window list on Windows: visible top-level windows, frontmost first,
//! in the editor's coordinates.

use super::from_wide;
use crate::windows::Window;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetClassNameW, GetWindowLongW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    WS_EX_TOOLWINDOW,
};
use windows_sys::core::BOOL;

/// `scale`: physical pixels per editor unit.
pub fn list(scale: f64) -> Vec<Window> {
    let mut hwnds: Vec<HWND> = Vec::new();
    unsafe extern "system" fn each(h: HWND, data: LPARAM) -> BOOL {
        unsafe { (*(data as *mut Vec<HWND>)).push(h) };
        1
    }
    unsafe { EnumWindows(Some(each), &mut hwnds as *mut _ as LPARAM) };
    let me = std::process::id();
    let s = scale as f32;
    let wins: Vec<Window> = hwnds
        .into_iter()
        .filter_map(|h| {
            unsafe {
                if IsWindowVisible(h) == 0 || IsIconic(h) != 0 || GetWindowLongW(h, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0 {
                    return None;
                }
            }
            // Windows on other virtual desktops, and suspended Store apps,
            // are "cloaked": visible to EnumWindows but not on screen.
            let mut cloaked = 0u32;
            unsafe { DwmGetWindowAttribute(h, DWMWA_CLOAKED as u32, (&mut cloaked as *mut u32).cast(), 4) };
            if cloaked != 0 {
                return None;
            }
            let class = text(|b| unsafe { GetClassNameW(h, b.as_mut_ptr(), b.len() as i32) });
            // The desktop and the taskbar.
            if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
                return None;
            }
            let mut pid = 0;
            unsafe { GetWindowThreadProcessId(h, &mut pid) };
            if pid == me {
                return None;
            }
            // The visible frame, without the invisible resize borders
            // GetWindowRect includes.
            let mut r: RECT = unsafe { std::mem::zeroed() };
            let hr = unsafe {
                DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS as u32, (&mut r as *mut RECT).cast(), size_of::<RECT>() as u32)
            };
            if hr != 0 {
                return None;
            }
            Some(Window {
                x: r.left as f32 / s,
                y: r.top as f32 / s,
                width: (r.right - r.left) as f32 / s,
                height: (r.bottom - r.top) as f32 / s,
                app: app_name(pid),
                title: text(|b| unsafe { GetWindowTextW(h, b.as_mut_ptr(), b.len() as i32) }),
                id: (h as usize).to_string(),
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

fn text(get: impl FnOnce(&mut [u16]) -> i32) -> String {
    let mut buf = [0u16; 512];
    let n = get(&mut buf).max(0) as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

/// The program's file name without ".exe": "firefox", "Code", "explorer".
fn app_name(pid: u32) -> String {
    unsafe {
        let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if p.is_null() {
            return String::new();
        }
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(p, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut n);
        CloseHandle(p);
        if ok == 0 {
            return String::new();
        }
        let path = from_wide(&buf[..n as usize]);
        let file = path.rsplit('\\').next().unwrap_or(&path);
        file.strip_suffix(".exe").or_else(|| file.strip_suffix(".EXE")).unwrap_or(file).to_string()
    }
}
