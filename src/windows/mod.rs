//! Where the windows are, so the overlay can highlight the one under the
//! pointer and select it with a click.
//!
//! Wayland has no protocol for this, so each compositor is asked in its own
//! way: KWin through a throwaway script, Sway and Hyprland over their IPC
//! sockets, X11 through the EWMH window list, and GNOME (which doesn't tell
//! applications where windows are) through clipt's optional GNOME Shell
//! extension (data/gnome-extension). Anything else, or GNOME without the
//! extension, returns an empty list and the overlay falls back to
//! highlighting whole monitors. macOS lists its windows in
//! `macos::windows`.
//!
//! Positions are in the same logical desktop coordinates as the outputs. The
//! list is topmost first.

#[cfg(wayland_x11)]
mod linux;
#[cfg(wayland_x11)]
pub use linux::{install_gnome_extension, list};

#[derive(Clone, Debug)]
pub struct Window {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// App id / WM_CLASS, for the label when there's no title.
    pub app: String,
    pub title: String,
    /// The compositor's own id, for finding the window again while
    /// recording it: Sway's container id, Hyprland's address, KWin's
    /// internal id (a UUID), the X11 window id. Empty on GNOME.
    pub id: String,
}

impl Window {
    /// What the hover label shows: the app, since titles are often long.
    pub fn name(&self) -> &str {
        if self.app.is_empty() { &self.title } else { &self.app }
    }
}
