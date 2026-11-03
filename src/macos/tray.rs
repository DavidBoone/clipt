//! `clipt tray` on macOS: an icon in the menu bar with the Linux tray's
//! screenshot actions, and global shortcuts. Opening clipt.app starts it.
//!
//! As on Linux, the tray never captures anything itself: each action starts
//! a fresh `clipt`, which, being the same executable, macOS counts as the
//! same app for the Screen Recording permission.
//!
//! The shortcuts use Carbon's `RegisterEventHotKey`, which, unlike watching
//! every key press, needs no Accessibility permission.

use crate::config::Config;
use anyhow::{Context, Result, bail};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, ProtocolObject};
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSControlStateValueOff, NSControlStateValueOn,
    NSEventModifierFlags, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSVariableStatusItemLength,
};
use objc2_foundation::{NSBundle, NSData, NSObject, NSObjectProtocol, NSSize, NSString};
use std::ffi::c_void;
use std::process::Command;
use std::sync::OnceLock;

/// Long enough for the menu to fade out of the shot.
const CLICK_DELAY: &str = "0.25";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Region,
    RegionDelayed,
    Full,
    OpenFolder,
    Login,
    Quit,
}

/// (label, action); `None` is a separator. A menu item's tag is its index.
const MENU: &[Option<(&str, Action)>] = &[
    Some(("Capture Region", Action::Region)),
    Some(("Capture Region in 3 Seconds", Action::RegionDelayed)),
    Some(("Copy Full Desktop", Action::Full)),
    None,
    Some(("Open Screenshots Folder", Action::OpenFolder)),
    None,
    Some(("Open at Login", Action::Login)),
    Some(("Quit clipt", Action::Quit)),
];

/// The screenshots folder, for the hotkey handler, which gets no context.
static SAVE_DIR: OnceLock<String> = OnceLock::new();

fn run_action(a: Action) {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return eprintln!("clipt: {e}"),
    };
    let r = match a {
        Action::Region => Command::new(&exe).args(["gui", "-d", CLICK_DELAY]).spawn(),
        Action::RegionDelayed => Command::new(&exe).args(["gui", "-d", "3"]).spawn(),
        Action::Full => Command::new(&exe).args(["full", "-c", "-d", CLICK_DELAY]).spawn(),
        Action::OpenFolder => {
            let dir = SAVE_DIR.get().map(String::as_str).unwrap_or("~");
            let _ = std::fs::create_dir_all(dir);
            Command::new("/usr/bin/open").arg(dir).spawn()
        }
        // Handled by the menu, which has the item to tick.
        Action::Login | Action::Quit => return,
    };
    // Reap it in the background, so it doesn't linger as a zombie.
    match r {
        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
        Err(e) => eprintln!("clipt: starting {a:?}: {e}"),
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CliptTray"]
    struct Tray;

    unsafe impl NSObjectProtocol for Tray {}

    unsafe impl NSApplicationDelegate for Tray {
        /// Opening clipt.app again while the tray runs captures a region.
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn should_handle_reopen(&self, _app: &NSApplication, _visible: bool) -> bool {
            run_action(Action::Region);
            false
        }
    }

    impl Tray {
        #[unsafe(method(act:))]
        fn act(&self, item: &NSMenuItem) {
            let Some(Some((_, a))) = MENU.get(item.tag() as usize) else { return };
            match a {
                Action::Login => toggle_login(item),
                Action::Quit => NSApplication::sharedApplication(self.mtm()).terminate(None),
                a => run_action(*a),
            }
        }
    }
);

impl Tray {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

pub fn run(cfg: &Config) -> Result<()> {
    let mtm = MainThreadMarker::new().context("not on the main thread")?;
    let _ = SAVE_DIR.set(crate::config::expand(&cfg.save_dir).display().to_string());
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let tray = Tray::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*tray)));

    let hotkeys = [(cfg.hotkey.as_str(), Action::Region), (cfg.hotkey_full.as_str(), Action::Full)];
    let mut region_key = None;
    for (spec, action) in hotkeys {
        if spec.is_empty() {
            continue;
        }
        let key = Hotkey::parse(spec).with_context(|| format!("hotkey '{spec}'"))?;
        if let Err(e) = hotkey::register(&key, action) {
            // Most likely another app holds the same shortcut.
            eprintln!("clipt: couldn't register the shortcut {spec}: {e:#}");
            continue;
        }
        if action == Action::Region {
            region_key = Some(key);
        }
    }

    let menu = NSMenu::new(mtm);
    for (i, entry) in MENU.iter().enumerate() {
        let Some((label, action)) = entry else {
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            continue;
        };
        if *action == Action::Login && !login_supported() {
            continue;
        }
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(label),
                Some(sel!(act:)),
                &NSString::new(),
            )
        };
        unsafe { item.setTarget(Some(&tray)) };
        item.setTag(i as isize);
        // Shown beside the item, as other menu bar apps show theirs.
        if let (Action::Region, Some(k)) = (action, &region_key) {
            item.setKeyEquivalent(&NSString::from_str(&k.equivalent));
            item.setKeyEquivalentModifierMask(k.modifier_flags());
        }
        if *action == Action::Login {
            item.setState(if login_enabled() { NSControlStateValueOn } else { NSControlStateValueOff });
        }
        menu.addItem(&item);
    }

    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = item.button(mtm) {
        button.setImage(Some(&*menu_bar_icon()?));
        button.setToolTip(Some(&NSString::from_str("clipt")));
    }
    item.setMenu(Some(&menu));

    app.run();
    Ok(())
}

/// The app icon at menu bar size: 18 points, drawn at 2x for Retina.
fn menu_bar_icon() -> Result<Retained<NSImage>> {
    let png = crate::icon::png(36)?;
    let data = NSData::with_bytes(&png);
    let img = NSImage::initWithData(NSImage::alloc(), &data).context("the menu bar icon didn't decode")?;
    img.setSize(NSSize::new(18.0, 18.0));
    Ok(img)
}

/// Whether this is clipt.app, which can open at login, and macOS 13 or
/// later, which has `SMAppService`.
fn login_supported() -> bool {
    NSBundle::mainBundle().bundleIdentifier().is_some() && AnyClass::get(c"SMAppService").is_some()
}

fn login_enabled() -> bool {
    use objc2_service_management::{SMAppService, SMAppServiceStatus};
    login_supported() && unsafe { SMAppService::mainAppService().status() } == SMAppServiceStatus::Enabled
}

fn toggle_login(item: &NSMenuItem) {
    use objc2_service_management::SMAppService;
    if !login_supported() {
        return;
    }
    let svc = unsafe { SMAppService::mainAppService() };
    let r = if login_enabled() { unsafe { svc.unregisterAndReturnError() } } else { unsafe { svc.registerAndReturnError() } };
    if let Err(e) = r {
        eprintln!("clipt: open at login: {}", e.localizedDescription());
    }
    item.setState(if login_enabled() { NSControlStateValueOn } else { NSControlStateValueOff });
}

/// A shortcut from the config, such as "cmd+shift+2".
#[derive(Debug, PartialEq)]
struct Hotkey {
    /// The key's virtual key code (ANSI layout).
    code: u32,
    cmd: bool,
    shift: bool,
    option: bool,
    control: bool,
    /// The key as an NSMenuItem key equivalent, lower case.
    equivalent: String,
}

impl Hotkey {
    fn parse(spec: &str) -> Result<Self> {
        let mut k = Hotkey { code: 0, cmd: false, shift: false, option: false, control: false, equivalent: String::new() };
        let mut key = None;
        for part in spec.split('+').map(|p| p.trim().to_ascii_lowercase()) {
            match part.as_str() {
                "cmd" | "command" | "⌘" => k.cmd = true,
                "shift" | "⇧" => k.shift = true,
                "opt" | "option" | "alt" | "⌥" => k.option = true,
                "ctrl" | "control" | "⌃" => k.control = true,
                "" => bail!("an empty key"),
                other if key.is_none() => key = Some(other.to_string()),
                other => bail!("two keys ('{}' and '{other}'); one key and modifiers", key.unwrap_or_default()),
            }
        }
        let key = key.context("no key, only modifiers")?;
        if !(k.cmd || k.option || k.control) && !key.starts_with('f') {
            bail!("needs cmd, option or ctrl, or it would take that key from every app");
        }
        k.code = key_code(&key).with_context(|| format!("unknown key '{key}' (a-z, 0-9, f1-f12, space, or punctuation)"))?;
        k.equivalent = match key.as_str() {
            "space" => " ".into(),
            // Function keys show in menus by their private-use characters.
            f if f.len() > 1 => {
                f.trim_start_matches('f').parse::<u32>().ok().and_then(|n| char::from_u32(0xF703 + n)).map(String::from).unwrap_or_default()
            }
            c => c.into(),
        };
        Ok(k)
    }

    fn modifier_flags(&self) -> NSEventModifierFlags {
        let mut f = NSEventModifierFlags::empty();
        for (on, flag) in [
            (self.cmd, NSEventModifierFlags::Command),
            (self.shift, NSEventModifierFlags::Shift),
            (self.option, NSEventModifierFlags::Option),
            (self.control, NSEventModifierFlags::Control),
        ] {
            if on {
                f |= flag;
            }
        }
        f
    }

    /// Carbon's modifier mask (cmdKey, shiftKey, optionKey, controlKey).
    fn carbon_modifiers(&self) -> u32 {
        (self.cmd as u32) << 8 | (self.shift as u32) << 9 | (self.option as u32) << 11 | (self.control as u32) << 12
    }
}

/// Virtual key codes (Events.h, kVK_ANSI_*), which name positions on the
/// keyboard: on other layouts a letter names the key where ANSI has it.
fn key_code(key: &str) -> Option<u32> {
    const CODES: &[(&str, u32)] = &[
        ("a", 0),
        ("s", 1),
        ("d", 2),
        ("f", 3),
        ("h", 4),
        ("g", 5),
        ("z", 6),
        ("x", 7),
        ("c", 8),
        ("v", 9),
        ("b", 11),
        ("q", 12),
        ("w", 13),
        ("e", 14),
        ("r", 15),
        ("y", 16),
        ("t", 17),
        ("1", 18),
        ("2", 19),
        ("3", 20),
        ("4", 21),
        ("6", 22),
        ("5", 23),
        ("=", 24),
        ("9", 25),
        ("7", 26),
        ("-", 27),
        ("8", 28),
        ("0", 29),
        ("]", 30),
        ("o", 31),
        ("u", 32),
        ("[", 33),
        ("i", 34),
        ("p", 35),
        ("l", 37),
        ("j", 38),
        ("'", 39),
        ("k", 40),
        (";", 41),
        ("\\", 42),
        (",", 43),
        ("/", 44),
        ("n", 45),
        ("m", 46),
        (".", 47),
        ("space", 49),
        ("`", 50),
        ("f1", 122),
        ("f2", 120),
        ("f3", 99),
        ("f4", 118),
        ("f5", 96),
        ("f6", 97),
        ("f7", 98),
        ("f8", 100),
        ("f9", 101),
        ("f10", 109),
        ("f11", 103),
        ("f12", 111),
    ];
    CODES.iter().find(|(k, _)| *k == key).map(|(_, c)| *c)
}

/// Carbon's hot keys: still the way to take a global shortcut without the
/// Accessibility permission. NSApplication's run loop delivers their events.
mod hotkey {
    use super::{Action, Hotkey, c_void, run_action};
    use anyhow::{Result, bail};
    use std::sync::Mutex;

    #[repr(C)]
    struct EventTypeSpec {
        class: u32,
        kind: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct EventHotKeyId {
        signature: u32,
        id: u32,
    }

    type Handler = extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32;

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn GetApplicationEventTarget() -> *mut c_void;
        fn InstallEventHandler(
            target: *mut c_void,
            handler: Handler,
            count: usize,
            types: *const EventTypeSpec,
            user_data: *mut c_void,
            out: *mut *mut c_void,
        ) -> i32;
        fn RegisterEventHotKey(
            code: u32,
            modifiers: u32,
            id: EventHotKeyId,
            target: *mut c_void,
            options: u32,
            out: *mut *mut c_void,
        ) -> i32;
        fn GetEventParameter(
            event: *mut c_void,
            name: u32,
            desired_type: u32,
            actual_type: *mut u32,
            size: usize,
            actual_size: *mut usize,
            data: *mut c_void,
        ) -> i32;
    }

    const fn four(c: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*c)
    }
    const SIGNATURE: u32 = four(b"clpt");
    const EVENT_CLASS_KEYBOARD: u32 = four(b"keyb");
    const EVENT_HOT_KEY_PRESSED: u32 = 5;
    const PARAM_DIRECT_OBJECT: u32 = four(b"----");
    const TYPE_HOT_KEY_ID: u32 = four(b"hkid");

    /// What each registered hot key does, by its id.
    static ACTIONS: Mutex<Vec<Action>> = Mutex::new(Vec::new());

    extern "C" fn pressed(_call: *mut c_void, event: *mut c_void, _data: *mut c_void) -> i32 {
        let mut id = EventHotKeyId { signature: 0, id: 0 };
        let r = unsafe {
            GetEventParameter(
                event,
                PARAM_DIRECT_OBJECT,
                TYPE_HOT_KEY_ID,
                std::ptr::null_mut(),
                size_of::<EventHotKeyId>(),
                std::ptr::null_mut(),
                (&raw mut id).cast(),
            )
        };
        if r == 0 && id.signature == SIGNATURE {
            let action = ACTIONS.lock().ok().and_then(|a| a.get(id.id as usize).copied());
            if let Some(a) = action {
                run_action(a);
            }
        }
        0
    }

    pub(super) fn register(key: &Hotkey, action: Action) -> Result<()> {
        let mut actions = ACTIONS.lock().map_err(|_| anyhow::anyhow!("hot key table poisoned"))?;
        if actions.is_empty() {
            let spec = EventTypeSpec { class: EVENT_CLASS_KEYBOARD, kind: EVENT_HOT_KEY_PRESSED };
            let r =
                unsafe { InstallEventHandler(GetApplicationEventTarget(), pressed, 1, &spec, std::ptr::null_mut(), std::ptr::null_mut()) };
            if r != 0 {
                bail!("InstallEventHandler failed ({r})");
            }
        }
        let id = EventHotKeyId { signature: SIGNATURE, id: actions.len() as u32 };
        let mut out = std::ptr::null_mut();
        let r = unsafe { RegisterEventHotKey(key.code, key.carbon_modifiers(), id, GetApplicationEventTarget(), 0, &mut out) };
        if r != 0 {
            bail!("RegisterEventHotKey failed ({r})");
        }
        actions.push(action);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hotkeys() {
        let k = Hotkey::parse("cmd+shift+2").unwrap();
        assert_eq!((k.code, k.carbon_modifiers(), k.equivalent.as_str()), (19, 0x300, "2"));
        let k = Hotkey::parse("Ctrl + Option + A").unwrap();
        assert_eq!((k.code, k.carbon_modifiers()), (0, 0x1800));
        assert_eq!(Hotkey::parse("f5").unwrap().code, 96);
        assert!(Hotkey::parse("shift+2").is_err());
        assert!(Hotkey::parse("cmd+shift").is_err());
        assert!(Hotkey::parse("cmd+a+b").is_err());
        assert!(Hotkey::parse("cmd+nope").is_err());
    }
}
