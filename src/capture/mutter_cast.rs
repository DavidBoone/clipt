//! GNOME recording and screenshots: Mutter's own ScreenCast D-Bus API (what
//! the portal and gnome-remote-desktop use underneath) hands out a PipeWire
//! node for a monitor or an area, with no dialog. `capture::pipewire` reads
//! it.

use super::LogicalRect;
use super::pipewire::{CropFn, DmaBufUnreadable, Remote};
use super::{Output, PixelOrder, Shot};
use crate::capture::VideoFrame;
use crate::image::Image;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
use zbus::{OwnedObjectPath, Variant as Value};

const DEST: &str = "org.gnome.Mutter.ScreenCast";
const SESSION: &str = "org.gnome.Mutter.ScreenCast.Session";

#[derive(Clone)]
pub enum Target {
    /// A monitor, by connector name (the wl_output name).
    Monitor(String),
    /// An area of the desktop, in logical coordinates.
    Area(LogicalRect),
}

pub fn available(conn: &crate::dbus::Connection) -> bool {
    crate::dbus::name_has_owner(conn, DEST)
}

/// Record `target` until `sink` returns false.
pub fn stream(target: Target, cursor: bool, crop: CropFn, sink: Box<dyn FnMut(VideoFrame) -> bool>) -> Result<()> {
    let conn = crate::dbus::session().context("connecting to the session bus")?;
    let (session, node) = start(&conn, &target, cursor)?;
    let r = super::pipewire::stream(Remote { fd: None, node, linear_dmabuf: false, few_buffers: false }, crop, sink);
    stop(&conn, &session);
    r
}

/// Start a screencast session for `target`: its path (to stop it) and the
/// PipeWire node its frames come from.
fn start(conn: &crate::dbus::Connection, target: &Target, cursor: bool) -> Result<(OwnedObjectPath, u32)> {
    let s = begin(conn, target, cursor)?;
    let session = s.session.clone();
    Ok((session, s.node()?))
}

/// A session Mutter has been asked to start, its PipeWire node still to be
/// announced. Asking for several before waiting on any lets Mutter set
/// their streams up together.
struct Starting {
    session: OwnedObjectPath,
    added: crate::dbus::Signals,
}

impl Starting {
    fn node(mut self) -> Result<u32> {
        let msg = self.added.next().context("Mutter didn't announce the PipeWire stream")?;
        let (node,): (u32,) = msg.body().deserialize()?;
        debug(&format!("Mutter screencast node {node}"));
        Ok(node)
    }
}

fn begin(conn: &crate::dbus::Connection, target: &Target, cursor: bool) -> Result<Starting> {
    let sc = crate::dbus::Proxy::new(conn, DEST, "/org/gnome/Mutter/ScreenCast", "org.gnome.Mutter.ScreenCast")?;
    let session_path: OwnedObjectPath = sc.call("CreateSession", &(HashMap::<&str, Value>::new(),)).context("Mutter CreateSession")?;
    let session = crate::dbus::Proxy::new(conn, DEST, session_path.as_str(), SESSION)?;
    let mut props: HashMap<&str, Value> = HashMap::new();
    // cursor-mode: 0 hidden, 1 embedded
    props.insert("cursor-mode", Value::from(cursor as u32));
    let stream_path: OwnedObjectPath = match target {
        Target::Monitor(connector) => session.call("RecordMonitor", &(connector.as_str(), props)).context("Mutter RecordMonitor")?,
        &Target::Area((x, y, w, h)) => session
            .call("RecordArea", &(x.round() as i32, y.round() as i32, w.round().max(1.0) as i32, h.round().max(1.0) as i32, props))
            .context("Mutter RecordArea")?,
    };
    let stream = crate::dbus::Proxy::new(conn, DEST, stream_path.as_str(), "org.gnome.Mutter.ScreenCast.Stream")?;
    // Subscribe before starting, or the node can be announced before we listen.
    let added = stream.receive_signal("PipeWireStreamAdded")?;
    session.call::<_, ()>("Start", &()).context("Mutter Start")?;
    drop((session, stream));
    Ok(Starting { session: session_path, added })
}

fn stop(conn: &crate::dbus::Connection, session: &OwnedObjectPath) {
    let _ = crate::dbus::call_method(conn, DEST, session.as_str(), SESSION, "Stop", &());
}

/// The sessions a screenshot started, to stop them all once it has its
/// frames; `None` after that, so a retry starting late stops its own.
type Sessions = Arc<Mutex<Option<Vec<OwnedObjectPath>>>>;

/// Add `session` to those to stop, or stop it now if that has happened
/// already; false then.
fn track(conn: &crate::dbus::Connection, sessions: &Sessions, session: &OwnedObjectPath) -> bool {
    match sessions.lock().unwrap().as_mut() {
        Some(v) => {
            v.push(session.clone());
            true
        }
        None => {
            stop(conn, session);
            false
        }
    }
}

/// How long a screenshot waits for each monitor's first frame. It normally
/// takes under a quarter of a second.
const SHOT_TIMEOUT: Duration = Duration::from_millis(500);

/// Screenshot `outputs`: one stream per monitor, all at once, each stopped
/// after its first picture. Unlike the screenshot portal there's no flash,
/// no PNG written to ~/Pictures and read back, and each monitor comes at its
/// own resolution rather than the whole desktop at the highest scale (a 1x
/// monitor beside a 1.5x one isn't rendered at 1.5x and shrunk back).
///
/// Frames come as linear DMA-BUFs where Mutter can make them. In shared
/// memory Mutter sends a monitor only once it has painted it, and a monitor
/// it scans out directly isn't painted, so one could send nothing until its
/// content changed. A monitor that still sends nothing in time comes back
/// as `None`, for the caller to take from somewhere else.
pub fn capture(outputs: &[Output], cursor: bool) -> Result<Vec<Option<Shot>>> {
    super::pipewire::check()?;
    let t0 = Instant::now();
    let conn = crate::dbus::session().context("connecting to the session bus")?;
    let sessions: Sessions = Arc::new(Mutex::new(Some(Vec::with_capacity(outputs.len()))));
    let frames = first_frames(&conn, outputs, cursor, &sessions);
    // Stopping the sessions ends any stream still waiting for a frame, so
    // GNOME doesn't go on showing the screen as shared.
    let started = sessions.lock().unwrap().take().unwrap_or_default();
    for s in &started {
        stop(&conn, s);
    }
    let frames = frames?;
    let got = frames.iter().filter(|f| f.is_some()).count();
    debug(&format!("Mutter frames for {got} of {} monitors in {:.1} ms", outputs.len(), t0.elapsed().as_secs_f64() * 1e3));
    Ok(outputs.iter().zip(frames).map(|(o, f)| f.map(|f| shot(o, f))).collect())
}

/// Start a stream per output (recording each session in `sessions`), all
/// before waiting on any, and wait up to `SHOT_TIMEOUT` for their first
/// frames.
fn first_frames(conn: &crate::dbus::Connection, outputs: &[Output], cursor: bool, sessions: &Sessions) -> Result<Vec<Option<VideoFrame>>> {
    let mut starting = Vec::with_capacity(outputs.len());
    for o in outputs {
        let s = begin(conn, &Target::Monitor(o.name.clone()), cursor).with_context(|| format!("capturing {}", o.name))?;
        track(conn, sessions, &s.session);
        starting.push(s);
    }
    let (tx, rx) = mpsc::channel();
    for (i, (o, s)) in outputs.iter().zip(starting).enumerate() {
        let node = s.node().with_context(|| format!("capturing {}", o.name))?;
        let (tx, conn, sessions, name) = (tx.clone(), conn.clone(), sessions.clone(), o.name.clone());
        // Detached, not scoped: a stream that never delivers must not hold
        // the screenshot up past the timeout.
        std::thread::spawn(move || {
            let read = |node, linear_dmabuf| {
                let first = tx.clone();
                let sink = Box::new(move |f: VideoFrame| {
                    let _ = first.send((i, Ok(f)));
                    false
                });
                super::pipewire::stream(Remote { fd: None, node, linear_dmabuf, few_buffers: true }, Box::new(|_, _| None), sink)
            };
            let mut r = read(node, true);
            if r.as_ref().is_err_and(|e| e.is::<DmaBufUnreadable>()) {
                debug(&format!("{name}: DMA-BUF unreadable; asking for shared memory"));
                // On a new stream: the first one goes on sending DMA-BUFs to
                // whoever connects.
                r = begin(&conn, &Target::Monitor(name), cursor)
                    .and_then(|s| if track(&conn, &sessions, &s.session) { read(s.node()?, false) } else { Ok(()) });
            }
            if let Err(e) = r {
                let _ = tx.send((i, Err(e)));
            }
        });
    }
    drop(tx);
    let deadline = Instant::now() + SHOT_TIMEOUT;
    let mut frames: Vec<Option<VideoFrame>> = outputs.iter().map(|_| None).collect();
    // A monitor whose stream fails is left to the caller, like one that
    // times out; the others are still worth having.
    let mut waiting = outputs.len();
    while waiting > 0 {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok((i, Ok(f))) => frames[i] = Some(f),
            Ok((i, Err(e))) => debug(&format!("{}: {e:#}", outputs[i].name)),
            Err(_) => break,
        }
        waiting -= 1;
    }
    Ok(frames)
}

/// A frame as a screenshot of `o`, at the monitor's native size. Mutter
/// sends that size already; the resize only guards against it not doing so.
fn shot(o: &Output, f: VideoFrame) -> Shot {
    let mut data = f.data;
    for p in data.as_chunks_mut::<4>().0 {
        if f.order == PixelOrder::Bgrx {
            p.swap(0, 2);
        }
        p[3] = 255;
    }
    let mut image = Image { width: f.width, height: f.height, data };
    let (nw, nh) = o.native;
    if nw > 0 && nh > 0 && (nw, nh) != (image.width, image.height) {
        image = image.resize(nw, nh);
    }
    Shot { output: o.clone(), image }
}

fn debug(msg: &str) {
    if std::env::var_os("CLIPT_DEBUG").is_some() {
        eprintln!("clipt: {msg}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(native: (u32, u32)) -> Output {
        Output { index: 0, name: "DP-1".into(), description: String::new(), x: 0, y: 0, width: 2, height: 1, native }
    }

    #[test]
    fn frames_become_opaque_rgba() {
        let bgrx = VideoFrame { width: 2, height: 1, order: PixelOrder::Bgrx, data: vec![1, 2, 3, 0, 4, 5, 6, 9] };
        assert_eq!(shot(&output((2, 1)), bgrx).image.data, [3, 2, 1, 255, 6, 5, 4, 255]);
        let rgbx = VideoFrame { width: 2, height: 1, order: PixelOrder::Rgbx, data: vec![1, 2, 3, 0, 4, 5, 6, 9] };
        assert_eq!(shot(&output((2, 1)), rgbx).image.data, [1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn frames_come_out_at_the_native_size() {
        let f = VideoFrame { width: 2, height: 1, order: PixelOrder::Rgbx, data: vec![0; 8] };
        let s = shot(&output((4, 2)), f);
        assert_eq!((s.image.width, s.image.height), (4, 2));
    }
}
