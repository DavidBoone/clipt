//! clipt on Windows: screenshots, with the same editor as on Linux and
//! macOS. The tray, recording and `clipt update` aren't there yet; see
//! docs/windows-plan.md.

mod capture;
mod clipboard;
mod overlay;
mod windows;

use crate::config::Config;
use crate::editor::{Editor, Outcome};
use crate::{Args, crop_geometry, parse_geometry, stitch, write_png};
use anyhow::{Context, Result, bail};

pub const HELP: &str = "\
clipt — screenshot tool

USAGE:
    clipt [gui]           Select a region and annotate it (default)
    clipt full            Capture the whole desktop
    clipt screen [NAME]   Capture one monitor (default: the main one)
    clipt list            Show the monitors
    clipt config [KEY [VALUE]]  Show the settings, one setting, or set it

OPTIONS:
    -o, --output PATH     Write the PNG to PATH ('-' for stdout) instead of the save dir
    -c, --clipboard       Copy to the clipboard (full/screen)
    -g, --geometry GEOM   Crop to \"X,Y WxH\" (full)
    -d, --delay SECS      Wait before capturing
        --cursor          Include the pointer
    -h, --help            Show this help
    -V, --version         Show the version

EDITOR KEYS:
    drag                  select (click = whole screen; drag handles to resize)
    V R E A L P M T X N   select, rect, ellipse, arrow, line, pen, highlighter,
                          text, pixelate, counter
    1-8                   colour          scroll / [ ]   stroke width
    Shift                 square / 45°    arrows         nudge (Shift resizes)
    Enter, Ctrl+C, dbl-click  copy        Ctrl+S         save
    Ctrl+Z / Ctrl+Shift+Z undo / redo     Esc / right-click  cancel / back

Config: %APPDATA%\\clipt\\config.toml (save_dir, filename, color, palette,
width, copy_on_save, cursor, font). Screenshots go to
Pictures\\Screenshots, where Windows keeps its own.
";

pub fn run(args: Args, cfg: Config) -> Result<()> {
    match args.cmd.as_str() {
        "gui" | "full" | "screen" | "list" => {}
        "record" | "tray" | "update" | "gnome-extension" => bail!("`clipt {}` isn't available on Windows yet", args.cmd),
        other => bail!("unknown command '{other}' (try --help)"),
    }
    if args.backend.is_some() {
        bail!("--backend: Windows has only the one");
    }
    // Physical pixels everywhere, so every monitor is captured and covered
    // at its own resolution, whatever its scale.
    capture::dpi_aware();
    let cursor = args.cursor || cfg.cursor;
    if args.delay > 0.0 {
        std::thread::sleep(std::time::Duration::from_secs_f64(args.delay));
    }
    let desk = capture::Desktop::get()?;
    if args.cmd == "list" {
        for o in &desk.outputs {
            println!("{}\t{}x{}+{}+{}\t{}", o.name, o.native.0, o.native.1, o.x, o.y, o.description);
        }
        return Ok(());
    }

    if args.cmd != "gui" {
        let targets: Vec<usize> = if args.cmd == "screen" {
            match &args.screen {
                Some(n) => vec![
                    desk.outputs.iter().position(|o| &o.name == n).with_context(|| format!("no monitor named '{n}' (see clipt list)"))?,
                ],
                None => vec![0],
            }
        } else if let Some(g) = &args.geometry {
            let (x, y, w, h) = parse_geometry(g)?;
            let hit: Vec<_> = (0..desk.outputs.len())
                .filter(|&i| {
                    let o = &desk.outputs[i];
                    (o.x as f64) < x + w && x < (o.x + o.width as i32) as f64 && (o.y as f64) < y + h && y < (o.y + o.height as i32) as f64
                })
                .collect();
            anyhow::ensure!(!hit.is_empty(), "geometry '{g}' is outside every monitor");
            hit
        } else {
            (0..desk.outputs.len()).collect()
        };
        let shots = desk.capture(&targets, cursor)?;
        let mut img = stitch(&shots);
        if let Some(g) = &args.geometry {
            img = crop_geometry(&img, &shots, g)?;
        }
        let png = img.encode_png()?;
        let path = if args.output.is_some() || !args.clipboard { write_png(&cfg, args.output.as_deref(), &png)? } else { None };
        if let Some(p) = &path
            && args.output.is_none()
        {
            println!("{}", p.display());
        }
        if args.clipboard {
            clipboard::copy(&img, &png)?;
        }
        return Ok(());
    }

    // Fonts and the window list come while the screens are captured.
    let font_path = cfg.font.clone();
    let fonts = std::thread::spawn(move || crate::text::Fonts::load(font_path.as_deref()));
    let scale = desk.scale;
    let wins = std::thread::spawn(move || windows::list(scale));
    let shots = desk.capture(&(0..desk.outputs.len()).collect::<Vec<_>>(), cursor)?;
    let fonts = fonts.join().map_err(|_| anyhow::anyhow!("font loader panicked"))??;
    let mut ed = Editor::new(shots, fonts, true, cfg.color.saturating_sub(1), cfg.width);
    ed.windows = wins.join().unwrap_or_default();
    ed.palette = cfg.palette_rgb();

    let mut session = overlay::Session::new(ed, &desk)?;
    let outcome = session.run()?;
    if session.editor().palette_changed
        && let Err(e) = Config::save_palette(&session.editor().palette)
    {
        eprintln!("clipt: saving the palette: {e:#}");
    }
    if outcome == Outcome::Cancel {
        return Ok(());
    }
    let img = session.editor().export().context("nothing selected")?;
    session.hide();
    drop(session);
    let png = img.encode_png()?;
    if outcome == Outcome::Copy || cfg.copy_on_save {
        clipboard::copy(&img, &png)?;
    }
    if outcome == Outcome::Save || args.output.is_some() {
        write_png(&cfg, args.output.as_deref(), &png)?;
    }
    Ok(())
}

/// A null-terminated UTF-16 copy of `s`, for the W functions.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// A UTF-16 buffer up to its first null.
fn from_wide(s: &[u16]) -> String {
    String::from_utf16_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())])
}
