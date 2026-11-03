//! The editor on macOS: one borderless window per display above everything
//! else (the menu bar and the Dock too), each showing its screen through a
//! layer whose contents are a bitmap the editor paints, as on X11.
//!
//! clipt runs its own event loop instead of `NSApplication.run`, so that
//! `run` can return once the user has decided; key events are handled here
//! and not passed on, since no responder would take them and AppKit beeps.

use crate::editor::damage::{IRect, merge, screen_rects};
use crate::editor::shapes::RectExt;
use crate::editor::{CursorKind, Damage, Editor, Handle, Key, Mods, Outcome};
use anyhow::{Context, Result};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSCursor, NSEvent, NSEventMask, NSEventModifierFlags,
    NSEventType, NSScreenSaverWindowLevel, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::{CFData, CFRetained};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGDisplayBounds, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo,
    CGMainDisplayID, kCGColorSpaceSRGB,
};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize};
use objc2_quartz_core::{CALayer, CATransaction};

define_class!(
    // A borderless window can't take the keyboard unless it says so.
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    #[name = "CliptOverlayWindow"]
    struct OverlayWindow;

    impl OverlayWindow {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }
    }
);

struct Win {
    window: Retained<OverlayWindow>,
    layer: Retained<CALayer>,
    screen: usize,
    /// What the window shows, BGRX, at the display's native size.
    buf: Vec<u8>,
    full: bool,
}

pub struct Session {
    mtm: MainThreadMarker,
    app: Retained<NSApplication>,
    editor: Editor,
    wins: Vec<Win>,
    space: CFRetained<CGColorSpace>,
    /// Height of the main display, for turning AppKit's coordinates (y up
    /// from the main display's bottom) into the editor's (y down from its top).
    main_height: f64,
    current_cursor: Option<CursorKind>,
    /// Trackpads scroll in small steps; the editor takes whole ones.
    scroll: f64,
    dirty: bool,
}

impl Session {
    pub fn new(mut editor: Editor, mtm: MainThreadMarker) -> Result<Self> {
        let app = NSApplication::sharedApplication(mtm);
        // No Dock icon or menu bar, but allowed to take the keyboard.
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        app.finishLaunching();
        // The bitmaps are BGRX, the order Core Graphics draws fastest.
        editor.set_swap_rb(true);
        let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).context("no sRGB colour space")?;
        let main_height = CGDisplayBounds(CGMainDisplayID()).size.height;

        let mut wins = Vec::new();
        for (i, scr) in editor.screens.iter().enumerate() {
            let o = &scr.output;
            let frame = NSRect::new(
                NSPoint::new(o.x as f64, main_height - o.y as f64 - o.height as f64),
                NSSize::new(o.width as f64, o.height as f64),
            );
            let window: Retained<OverlayWindow> = unsafe {
                msg_send![
                    OverlayWindow::alloc(mtm),
                    initWithContentRect: frame,
                    styleMask: NSWindowStyleMask::Borderless,
                    backing: NSBackingStoreType::Buffered,
                    defer: false,
                ]
            };
            unsafe { window.setReleasedWhenClosed(false) };
            window.setLevel(NSScreenSaverWindowLevel);
            // Over full-screen apps, on whichever Space is showing.
            window.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary
                    | NSWindowCollectionBehavior::Stationary
                    | NSWindowCollectionBehavior::IgnoresCycle,
            );
            window.setOpaque(true);
            window.setHasShadow(false);
            window.setBackgroundColor(Some(&NSColor::blackColor()));
            window.setAcceptsMouseMovedEvents(true);
            window.disableCursorRects();
            // The frame again: AppKit keeps a new window below the menu bar.
            window.setFrame_display(frame, false);
            // Layer-hosting (the layer set before asking for one): AppKit
            // leaves the contents of a layer it didn't make alone.
            let view = window.contentView().context("overlay window has no view")?;
            let layer = CALayer::new();
            view.setLayer(Some(&layer));
            view.setWantsLayer(true);
            let len = (scr.bg.width() * scr.bg.height() * 4) as usize;
            wins.push(Win { window, layer, screen: i, buf: vec![0; len], full: true });
        }

        let mut s = Self { mtm, app, editor, wins, space, main_height, current_cursor: None, scroll: 0.0, dirty: true };
        // Painted before they're shown, so the screen doesn't flash.
        s.draw()?;
        for w in &s.wins {
            w.window.orderFrontRegardless();
        }
        #[allow(deprecated)] // its replacement, `activate`, is macOS 14+
        s.app.activateIgnoringOtherApps(true);
        // The key window is the one under the pointer, as a click would pick.
        let p = s.logical(NSEvent::mouseLocation());
        let under = s.wins.iter().position(|w| s.editor.screens[w.screen].bounds().contains(p.0, p.1)).unwrap_or(0);
        s.wins[under].window.makeKeyAndOrderFront(None);
        s.dirty |= s.editor.pointer_motion(p, Mods::default());
        Ok(s)
    }

    fn logical(&self, p: NSPoint) -> (f32, f32) {
        (p.x as f32, (self.main_height - p.y) as f32)
    }

    /// Where a mouse event happened, in the editor's coordinates.
    fn location(&self, ev: &NSEvent) -> (f32, f32) {
        match ev.window(self.mtm) {
            Some(w) => self.logical(w.convertPointToScreen(ev.locationInWindow())),
            None => self.logical(NSEvent::mouseLocation()),
        }
    }

    /// Returns whether AppKit should see the event too.
    fn handle(&mut self, ev: &NSEvent) -> bool {
        let flags = ev.modifierFlags();
        let mods = Mods {
            // ⌘ does what Ctrl does elsewhere (⌘C, ⌘S, ⌘Z); Ctrl still works.
            ctrl: flags.intersects(NSEventModifierFlags::Command | NSEventModifierFlags::Control),
            shift: flags.contains(NSEventModifierFlags::Shift),
            alt: flags.contains(NSEventModifierFlags::Option),
        };
        let (changed, forward) = match ev.r#type() {
            NSEventType::MouseMoved | NSEventType::LeftMouseDragged | NSEventType::RightMouseDragged => {
                (self.editor.pointer_motion(self.location(ev), mods), true)
            }
            NSEventType::LeftMouseDown => (self.editor.button_press(self.location(ev), true, mods), true),
            NSEventType::LeftMouseUp => (self.editor.button_release(self.location(ev), true), true),
            NSEventType::RightMouseDown => (self.editor.button_press(self.location(ev), false, mods), true),
            NSEventType::RightMouseUp => (self.editor.button_release(self.location(ev), false), true),
            NSEventType::ScrollWheel => {
                // Wheels send whole lines; trackpads send points, about ten
                // to a line.
                let dy = ev.scrollingDeltaY() / if ev.hasPreciseScrollingDeltas() { 10.0 } else { 1.0 };
                self.scroll += dy;
                let mut changed = false;
                while self.scroll.abs() >= 1.0 {
                    let step = self.scroll.signum();
                    self.scroll -= step;
                    // Up (a positive delta) is the X11 wheel's button 4: -1.
                    changed |= self.editor.scroll(-step as f32);
                }
                (changed, false)
            }
            NSEventType::KeyDown => (key(ev).is_some_and(|k| self.editor.key(k, mods)), false),
            NSEventType::KeyUp | NSEventType::FlagsChanged => (false, false),
            _ => (false, true),
        };
        self.dirty |= changed;
        forward
    }

    fn draw(&mut self) -> Result<()> {
        let damage = self.editor.take_damage();
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        for i in 0..self.wins.len() {
            let scr_idx = self.wins[i].screen;
            let (w, h) = (self.editor.screens[scr_idx].bg.width(), self.editor.screens[scr_idx].bg.height());
            let rects = match (&damage, self.wins[i].full) {
                (Damage::Rects(rs), false) => Some(merge(screen_rects(&self.editor.screens[scr_idx], rs))),
                _ => None,
            };
            if rects.as_ref().is_some_and(|r| r.is_empty()) {
                continue;
            }
            let win = &mut self.wins[i];
            match rects {
                None => {
                    if let Some(mut pm) = tiny_skia::PixmapMut::from_bytes(&mut win.buf, w, h) {
                        crate::editor::ui::render(&self.editor, scr_idx, &mut pm, (0, 0));
                    }
                }
                Some(rs) => paint_rects(&self.editor, scr_idx, &mut win.buf, w, &rs),
            }
            win.full = false;
            let img = bitmap(&win.buf, w, h, &self.space).context("creating the overlay image")?;
            // A CGImage is what a layer's contents are documented to take.
            unsafe { win.layer.setContents(Some(&*(&*img as *const CGImage as *const AnyObject))) };
        }
        CATransaction::commit();
        self.dirty = false;
        Ok(())
    }

    fn update_cursor(&mut self) {
        let want = self.editor.cursor();
        if self.current_cursor == Some(want) {
            return;
        }
        // The resize cursors are deprecated in macOS 15, whose replacements
        // older releases lack.
        #[allow(deprecated)]
        let c = match want {
            CursorKind::Crosshair => NSCursor::crosshairCursor(),
            CursorKind::Move => NSCursor::openHandCursor(),
            CursorKind::Pointer => NSCursor::pointingHandCursor(),
            CursorKind::Text => NSCursor::IBeamCursor(),
            CursorKind::Resize(Handle::N | Handle::S) => NSCursor::resizeUpDownCursor(),
            CursorKind::Resize(Handle::E | Handle::W) => NSCursor::resizeLeftRightCursor(),
            // AppKit has no public diagonal resize cursor before macOS 15.
            CursorKind::Resize(_) => NSCursor::crosshairCursor(),
        };
        c.set();
        self.current_cursor = Some(want);
    }

    pub fn run(&mut self) -> Result<Outcome> {
        loop {
            if self.dirty || self.wins.iter().any(|w| w.full) {
                self.draw()?;
            }
            self.update_cursor();
            // Animations tick at about 120 Hz; otherwise sleep until input.
            let until = if self.editor.animating() { NSDate::dateWithTimeIntervalSinceNow(1.0 / 120.0) } else { NSDate::distantFuture() };
            let mut next = Some(until);
            while let Some(until) = next.take() {
                let ev = self.app.nextEventMatchingMask_untilDate_inMode_dequeue(
                    NSEventMask::Any,
                    Some(&until),
                    unsafe { NSDefaultRunLoopMode },
                    true,
                );
                let Some(ev) = ev else {
                    // Timed out: the animation moves on.
                    self.dirty |= self.editor.animating();
                    break;
                };
                if self.handle(&ev) {
                    self.app.sendEvent(&ev);
                }
                // Whatever else is queued, before the next frame.
                next = Some(NSDate::distantPast());
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
            w.window.orderOut(None);
            w.window.close();
        }
        NSCursor::arrowCursor().set();
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

/// An image of the bitmap as it is now. The copy is what lets the next frame
/// be painted while the window server may still be reading this one.
fn bitmap(buf: &[u8], w: u32, h: u32, space: &CGColorSpace) -> Option<CFRetained<CGImage>> {
    let data = CFData::from_bytes(buf);
    let provider = CGDataProvider::with_cf_data(Some(&data))?;
    // Bytes B, G, R, then one unused: a little-endian 32-bit xRGB.
    let info = CGBitmapInfo(CGImageAlphaInfo::NoneSkipFirst.0 | CGImageByteOrderInfo::Order32Little.0);
    unsafe {
        CGImage::new(
            w as usize,
            h as usize,
            8,
            32,
            w as usize * 4,
            Some(space),
            info,
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
}

/// The editor's key for a key press: special keys by their (layout
/// independent) codes, the rest by the character the layout gives.
fn key(ev: &NSEvent) -> Option<Key> {
    Some(match ev.keyCode() {
        53 => Key::Escape,
        36 | 76 => Key::Enter,
        51 => Key::Backspace,
        123 => Key::Left,
        124 => Key::Right,
        125 => Key::Down,
        126 => Key::Up,
        48 => Key::Tab,
        _ => {
            // Ignores every modifier but Shift, so ⌘C arrives as 'c'.
            let c = ev.charactersIgnoringModifiers()?.to_string().chars().next()?;
            if c.is_control() {
                return None;
            }
            Key::Char(c)
        }
    })
}
