//! The editor on Windows: one borderless topmost window per monitor, each
//! showing its screen from a DIB section the editor paints, as on X11 and
//! macOS. Damaged rectangles are repainted into the DIB and blitted.
//!
//! The window procedure only queues what happened; the loop in `run` hands
//! it to the editor, so the editor is never borrowed from inside a callback.

use super::capture::{Desktop, Dib};
use super::wide;
use crate::editor::damage::{IRect, merge, screen_rects};
use crate::editor::{CursorKind, Damage, Editor, Handle, Key, Mods, Outcome};
use anyhow::{Context, Result};
use std::cell::{Cell, RefCell};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_TRANSITIONS_FORCEDISABLED, DwmSetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{BeginPaint, BitBlt, EndPaint, GdiFlush, GetDC, PAINTSTRUCT, ReleaseDC, SRCCOPY};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_MENU, VK_RETURN,
    VK_RIGHT, VK_SHIFT, VK_TAB, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, HCURSOR, HTCLIENT, IDC_ARROW, IDC_CROSS, IDC_HAND,
    IDC_IBEAM, IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE, LoadCursorW, MSG, MsgWaitForMultipleObjects, PM_REMOVE,
    PeekMessageW, QS_ALLINPUT, RegisterClassExW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SetCursor, SetForegroundWindow, SetWindowPos,
    ShowWindow, TranslateMessage, WM_CHAR, WM_CLOSE, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR, WM_SYSCHAR, WM_SYSKEYDOWN, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP,
};

enum Ev {
    Motion(HWND, i32, i32),
    Button(HWND, i32, i32, bool, bool),
    Wheel(i16),
    KeyDown(u16),
    Char(char),
    Paint(HWND),
    Close,
}

thread_local! {
    static EVENTS: RefCell<Vec<Ev>> = const { RefCell::new(Vec::new()) };
    static CURSOR: Cell<HCURSOR> = const { Cell::new(std::ptr::null_mut()) };
    /// The first half of a character outside the Basic Multilingual Plane.
    static HIGH: Cell<u16> = const { Cell::new(0) };
}

fn push(e: Ev) {
    EVENTS.with_borrow_mut(|q| q.push(e));
}

fn xy(l: LPARAM) -> (i32, i32) {
    ((l & 0xffff) as u16 as i16 as i32, ((l >> 16) & 0xffff) as u16 as i16 as i32)
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let (x, y) = xy(l);
            push(Ev::Motion(h, x, y));
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONUP => {
            let (x, y) = xy(l);
            let down = matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN);
            // A drag keeps reporting to this window past its monitor's edge.
            unsafe {
                if down {
                    SetCapture(h);
                } else {
                    ReleaseCapture();
                }
            }
            push(Ev::Button(h, x, y, matches!(msg, WM_LBUTTONDOWN | WM_LBUTTONUP), down));
        }
        WM_MOUSEWHEEL => push(Ev::Wheel(((w >> 16) & 0xffff) as u16 as i16)),
        // The system variants too, so Alt and F10 don't open a window menu.
        WM_KEYDOWN | WM_SYSKEYDOWN => push(Ev::KeyDown(w as u16)),
        WM_CHAR | WM_SYSCHAR => {
            let u = w as u16;
            if (0xd800..0xdc00).contains(&u) {
                HIGH.set(u);
            } else {
                let c = if (0xdc00..0xe000).contains(&u) {
                    char::decode_utf16([HIGH.replace(0), u]).next().and_then(|r| r.ok())
                } else {
                    char::from_u32(u as u32)
                };
                if let Some(c) = c {
                    push(Ev::Char(c));
                }
            }
        }
        WM_SETCURSOR if (l & 0xffff) as u32 == HTCLIENT => {
            unsafe { SetCursor(CURSOR.get()) };
            return 1;
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = unsafe { std::mem::zeroed() };
            unsafe {
                BeginPaint(h, &mut ps);
                EndPaint(h, &ps);
            }
            push(Ev::Paint(h));
        }
        WM_ERASEBKGND => return 1,
        // Alt+F4.
        WM_CLOSE => push(Ev::Close),
        _ => return unsafe { DefWindowProcW(h, msg, w, l) },
    }
    0
}

struct Win {
    hwnd: HWND,
    screen: usize,
    /// The window's top left on the virtual desktop, in physical pixels.
    origin: (i32, i32),
    /// What the window shows, BGRX, at the monitor's physical size.
    dib: Dib,
    full: bool,
}

pub struct Session {
    editor: Editor,
    wins: Vec<Win>,
    scale: f64,
    current_cursor: Option<CursorKind>,
    dirty: bool,
}

const CLASS: &str = "CliptOverlay";

impl Session {
    pub fn new(mut editor: Editor, desk: &Desktop) -> Result<Self> {
        // DIBs are BGRX.
        editor.set_swap_rb(true);
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        let class = wide(CLASS);
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..unsafe { std::mem::zeroed() }
        };
        // Fails harmlessly if a second session registers it again.
        unsafe { RegisterClassExW(&wc) };
        CURSOR.set(unsafe { LoadCursorW(std::ptr::null_mut(), IDC_CROSS) });

        let mut wins = Vec::new();
        for (i, scr) in editor.screens.iter().enumerate() {
            let r = desk.phys[scr.output.index];
            let (w, h) = (scr.bg.width() as i32, scr.bg.height() as i32);
            let title = wide("clipt");
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                    class.as_ptr(),
                    title.as_ptr(),
                    WS_POPUP,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    instance,
                    std::ptr::null(),
                )
            };
            anyhow::ensure!(!hwnd.is_null(), "creating the overlay window failed");
            // No fade in: the frozen screen should simply be there.
            let off: i32 = 1;
            unsafe { DwmSetWindowAttribute(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED as u32, (&off as *const i32).cast(), 4) };
            wins.push(Win { hwnd, screen: i, origin: (r.left, r.top), dib: Dib::new(w, h).context("overlay bitmap")?, full: true });
        }

        let mut s = Self { editor, wins, scale: desk.scale, current_cursor: None, dirty: true };
        // Painted before they're shown, so the screen doesn't flash.
        s.draw();
        for w in &s.wins {
            unsafe {
                ShowWindow(w.hwnd, SW_SHOWNOACTIVATE);
                SetWindowPos(w.hwnd, std::ptr::null_mut(), 0, 0, 0, 0, SWP_NOACTIVATE | 0x0001 | 0x0002 | 0x0004);
            }
        }
        // The keyboard goes to the window under the pointer, as a click would pick.
        let mut p = POINT { x: 0, y: 0 };
        unsafe { GetCursorPos(&mut p) };
        let under = s
            .wins
            .iter()
            .position(|w| {
                let r = desk.phys[s.editor.screens[w.screen].output.index];
                p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
            })
            .unwrap_or(0);
        unsafe {
            SetForegroundWindow(s.wins[under].hwnd);
            SetFocus(s.wins[under].hwnd);
        }
        let lp = s.logical(p.x, p.y);
        s.dirty |= s.editor.pointer_motion(lp, Mods::default());
        Ok(s)
    }

    /// A point on the virtual desktop in physical pixels, in the editor's
    /// coordinates.
    fn logical(&self, x: i32, y: i32) -> (f32, f32) {
        ((x as f64 / self.scale) as f32, (y as f64 / self.scale) as f32)
    }

    fn at(&self, h: HWND, x: i32, y: i32) -> (f32, f32) {
        let (ox, oy) = self.wins.iter().find(|w| w.hwnd == h).map_or((0, 0), |w| w.origin);
        self.logical(ox + x, oy + y)
    }

    fn handle(&mut self, ev: Ev) {
        let down = |vk: u16| unsafe { GetKeyState(vk as i32) } < 0;
        let (ctrl, alt) = (down(VK_CONTROL), down(VK_MENU));
        // Ctrl+Alt is AltGr, which types characters.
        let mods = Mods { ctrl: ctrl && !alt, shift: down(VK_SHIFT), alt: alt && !ctrl };
        let changed = match ev {
            Ev::Motion(h, x, y) => {
                let p = self.at(h, x, y);
                self.editor.pointer_motion(p, mods)
            }
            Ev::Button(h, x, y, left, true) => {
                let p = self.at(h, x, y);
                self.editor.button_press(p, left, mods)
            }
            Ev::Button(h, x, y, left, false) => {
                let p = self.at(h, x, y);
                self.editor.button_release(p, left)
            }
            // 120 a notch; up (positive) is the X11 wheel's button 4: -1.
            Ev::Wheel(d) => {
                let mut changed = false;
                for _ in 0..(d.unsigned_abs() / 120).max(1) {
                    changed |= self.editor.scroll(-(d.signum() as f32));
                }
                changed
            }
            Ev::KeyDown(vk) => key(vk, mods).is_some_and(|k| self.editor.key(k, mods)),
            // Control characters come from Ctrl+letter, Enter, Tab and the
            // like, which KeyDown has handled.
            Ev::Char(c) if !c.is_control() && !mods.ctrl => self.editor.key(Key::Char(c), mods),
            Ev::Char(_) => false,
            Ev::Paint(h) => {
                if let Some(w) = self.wins.iter_mut().find(|w| w.hwnd == h) {
                    w.full = true;
                }
                false
            }
            Ev::Close => {
                self.editor.outcome = Some(Outcome::Cancel);
                false
            }
        };
        self.dirty |= changed;
    }

    fn draw(&mut self) {
        let damage = self.editor.take_damage();
        for i in 0..self.wins.len() {
            let idx = self.wins[i].screen;
            let full = self.wins[i].full;
            let scr = &self.editor.screens[idx];
            let (w, h) = (scr.bg.width(), scr.bg.height());
            let rects = match (&damage, full) {
                (Damage::Rects(rs), false) => merge(screen_rects(scr, rs)),
                _ => vec![IRect { x: 0, y: 0, w: w as i32, h: h as i32 }],
            };
            if rects.is_empty() {
                continue;
            }
            let win = &mut self.wins[i];
            let buf = unsafe { std::slice::from_raw_parts_mut(win.dib.bits, (w * h * 4) as usize) };
            if full || matches!(damage, Damage::Full) {
                if let Some(mut pm) = tiny_skia::PixmapMut::from_bytes(buf, w, h) {
                    crate::editor::ui::render(&self.editor, idx, &mut pm, (0, 0));
                }
            } else {
                paint_rects(&self.editor, idx, buf, w, &rects);
            }
            win.full = false;
            unsafe {
                GdiFlush();
                let dc = GetDC(win.hwnd);
                for r in &rects {
                    BitBlt(dc, r.x, r.y, r.w, r.h, win.dib.dc, r.x, r.y, SRCCOPY);
                }
                ReleaseDC(win.hwnd, dc);
            }
        }
        self.dirty = false;
    }

    fn update_cursor(&mut self) {
        let want = self.editor.cursor();
        if self.current_cursor == Some(want) {
            return;
        }
        let id = match want {
            CursorKind::Crosshair => IDC_CROSS,
            CursorKind::Move => IDC_SIZEALL,
            CursorKind::Pointer => IDC_HAND,
            CursorKind::Text => IDC_IBEAM,
            CursorKind::Resize(Handle::N | Handle::S) => IDC_SIZENS,
            CursorKind::Resize(Handle::E | Handle::W) => IDC_SIZEWE,
            CursorKind::Resize(Handle::NW | Handle::SE) => IDC_SIZENWSE,
            CursorKind::Resize(Handle::NE | Handle::SW) => IDC_SIZENESW,
        };
        let c = unsafe { LoadCursorW(std::ptr::null_mut(), id) };
        CURSOR.set(c);
        unsafe { SetCursor(c) };
        self.current_cursor = Some(want);
    }

    pub fn run(&mut self) -> Result<Outcome> {
        loop {
            if self.dirty || self.wins.iter().any(|w| w.full) {
                self.draw();
            }
            self.update_cursor();
            // Animations tick at about 120 Hz; otherwise sleep until input.
            let timeout = if self.editor.animating() { 8 } else { u32::MAX };
            let r = unsafe { MsgWaitForMultipleObjects(0, std::ptr::null(), 0, timeout, QS_ALLINPUT) };
            if r == 258 {
                // WAIT_TIMEOUT: the animation moves on.
                self.dirty |= self.editor.animating();
            }
            let mut msg: MSG = unsafe { std::mem::zeroed() };
            while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
                unsafe {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            for ev in EVENTS.with_borrow_mut(std::mem::take) {
                self.handle(ev);
            }
            if let Some(o) = self.editor.outcome {
                return Ok(o);
            }
        }
    }

    pub fn editor(&mut self) -> &mut Editor {
        &mut self.editor
    }

    pub fn hide(&mut self) {
        for w in self.wins.drain(..) {
            unsafe { DestroyWindow(w.hwnd) };
        }
        CURSOR.set(unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) });
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.hide();
    }
}

/// Repaint the damaged rectangles of one screen's bitmap.
fn paint_rects(ed: &Editor, idx: usize, buf: &mut [u8], w: u32, rs: &[IRect]) {
    for r in rs {
        let Some(mut tmp) = tiny_skia::Pixmap::new(r.w as u32, r.h as u32) else { continue };
        crate::editor::ui::render(ed, idx, &mut tmp.as_mut(), (r.x, r.y));
        let row = r.w as usize * 4;
        for (y, src) in tmp.data().chunks_exact(row).enumerate() {
            let off = ((r.y as usize + y) * w as usize + r.x as usize) * 4;
            buf[off..off + row].copy_from_slice(src);
        }
    }
}

/// The editor's key for a key press: special keys by virtual-key code, and
/// Ctrl+letter (whose WM_CHAR is a control character) by its letter. The
/// rest arrives as WM_CHAR, in the keyboard layout's characters.
fn key(vk: u16, mods: Mods) -> Option<Key> {
    Some(match vk {
        VK_ESCAPE => Key::Escape,
        VK_RETURN => Key::Enter,
        VK_BACK => Key::Backspace,
        VK_DELETE => Key::Delete,
        VK_LEFT => Key::Left,
        VK_RIGHT => Key::Right,
        VK_UP => Key::Up,
        VK_DOWN => Key::Down,
        VK_TAB => Key::Tab,
        0x41..=0x5a if mods.ctrl => {
            let c = vk as u8 as char;
            Key::Char(if mods.shift { c } else { c.to_ascii_lowercase() })
        }
        _ => return None,
    })
}
