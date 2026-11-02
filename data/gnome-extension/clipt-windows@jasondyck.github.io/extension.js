// clipt's helper for GNOME, for what GNOME Shell doesn't give applications:
//
// - Where windows are. Its own Introspect API answers only the
//   screen-sharing portal, and gives sizes, not positions, so clipt's overlay
//   could only highlight whole monitors. List gives the visible windows on
//   the current workspace, topmost first, as Mutter's frame rectangles, in
//   the layout coordinates clipt's monitors also use.
// - A frame around an area being recorded. Elsewhere clipt draws it with
//   layer-shell, which GNOME lacks. ShowFrame draws four thin strips just
//   outside the area, above everything and letting clicks through, so the
//   frame isn't in the video; red, or amber while paused. It goes with
//   HideFrame, or when the program that asked for it goes away.
//
// Window titles are what GNOME keeps from applications, so it answers only a
// caller whose program is named clipt. That stops casual snooping, not a
// determined program on your session.

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import St from 'gi://St';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const NAME = 'io.github.jasondyck.clipt.Windows';
const PATH = '/io/github/jasondyck/clipt/Windows';
const IFACE = `<node>
  <interface name="${NAME}">
    <!-- x, y, width, height (Mutter's layout coordinates), WM class, title,
         window id; topmost first. -->
    <method name="List">
      <arg type="a(iiiisss)" direction="out" name="windows"/>
    </method>
    <!-- The area being recorded (layout coordinates); paused turns it amber.
         Called again to move it or change its colour. -->
    <method name="ShowFrame">
      <arg type="i" direction="in" name="x"/>
      <arg type="i" direction="in" name="y"/>
      <arg type="i" direction="in" name="width"/>
      <arg type="i" direction="in" name="height"/>
      <arg type="b" direction="in" name="paused"/>
    </method>
    <method name="HideFrame"/>
  </interface>
</node>`;

const KINDS = [Meta.WindowType.NORMAL, Meta.WindowType.DIALOG, Meta.WindowType.MODAL_DIALOG, Meta.WindowType.UTILITY];
/** The frame's thickness, and its colours: clipt's own (src/record/indicator.rs). */
const T = 3;
// As CSS: Clutter.Color is gone from GNOME 47 on.
const RED = 'background-color: #ef4444;';
const AMBER = 'background-color: #f59e0b;';

function visibleWindows() {
    const workspace = global.workspace_manager.get_active_workspace();
    return global.display.sort_windows_by_stacking(global.display.list_all_windows())
        .filter(w => !w.minimized && !w.is_hidden() && !w.is_override_redirect() &&
            KINDS.includes(w.get_window_type()) &&
            (w.is_on_all_workspaces() || w.located_on_workspace(workspace)))
        .reverse()
        .map(w => {
            const r = w.get_frame_rect();
            return [r.x, r.y, r.width, r.height, w.get_wm_class() ?? '', w.get_title() ?? '', String(w.get_id())];
        });
}

/** Whether the sender's program is clipt (`/proc/PID/exe`). */
async function isClipt(invocation) {
    const reply = await invocation.get_connection().call(
        'org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus',
        'GetConnectionUnixProcessID', new GLib.Variant('(s)', [invocation.get_sender()]),
        new GLib.VariantType('(u)'), Gio.DBusCallFlags.NONE, 1000, null);
    const [pid] = reply.deepUnpack();
    try {
        const exe = GLib.file_read_link(`/proc/${pid}/exe`);
        return GLib.path_get_basename(exe).replace(/ \(deleted\)$/, '') === 'clipt';
    } catch {
        return false;
    }
}

/** The recording frame: four strips above the windows, not reactive, so
 *  clicks go to whatever is below. */
class Frame {
    constructor() {
        this._strips = [];
        this._watch = 0;
    }

    show(sender, x, y, w, h, paused) {
        if (this._strips.length === 0) {
            for (let i = 0; i < 4; i++) {
                const strip = new St.Widget({reactive: false});
                // The shell's top-level group: always shown (top_window_group
                // isn't, here), and added last, so above the windows.
                Main.layoutManager.uiGroup.add_child(strip);
                this._strips.push(strip);
            }
        }
        // Outside the area: above, below, left, right (the sides take the corners).
        const rects = [[x - T, y - T, w + 2 * T, T], [x - T, y + h, w + 2 * T, T], [x - T, y, T, h], [x + w, y, T, h]];
        this._strips.forEach((strip, i) => {
            const [sx, sy, sw, sh] = rects[i];
            strip.set_position(sx, sy);
            strip.set_size(sw, sh);
            strip.style = paused ? AMBER : RED;
        });
        // Gone with whoever asked for it, so a crash doesn't leave it up.
        if (this._watch === 0) {
            this._watch = Gio.bus_watch_name_on_connection(Gio.DBus.session, sender, Gio.BusNameWatcherFlags.NONE,
                null, () => this.hide());
        }
    }

    hide() {
        for (const strip of this._strips)
            strip.destroy();
        this._strips = [];
        if (this._watch !== 0) {
            Gio.bus_unwatch_name(this._watch);
            this._watch = 0;
        }
    }
}

class Service {
    constructor() {
        this.frame = new Frame();
    }

    /** Answer `invocation` with `answer()` if the caller is clipt, else refuse. */
    async _forClipt(invocation, answer) {
        let allowed = false;
        try {
            allowed = await isClipt(invocation);
        } catch (e) {
            logError(e, 'clipt helper');
        }
        if (!allowed) {
            invocation.return_dbus_error('org.freedesktop.DBus.Error.AccessDenied', 'only clipt may use this');
            return;
        }
        invocation.return_value(answer());
    }

    async ListAsync(_params, invocation) {
        await this._forClipt(invocation, () => new GLib.Variant('(a(iiiisss))', [visibleWindows()]));
    }

    async ShowFrameAsync([x, y, w, h, paused], invocation) {
        await this._forClipt(invocation, () => {
            this.frame.show(invocation.get_sender(), x, y, w, h, paused);
            return null;
        });
    }

    async HideFrameAsync(_params, invocation) {
        await this._forClipt(invocation, () => {
            this.frame.hide();
            return null;
        });
    }
}

export default class CliptWindows extends Extension {
    enable() {
        this._service = new Service();
        this._dbus = Gio.DBusExportedObject.wrapJSObject(IFACE, this._service);
        this._dbus.export(Gio.DBus.session, PATH);
        this._owner = Gio.bus_own_name_on_connection(Gio.DBus.session, NAME, Gio.BusNameOwnerFlags.NONE, null, null);
    }

    disable() {
        this._service.frame.hide();
        Gio.bus_unown_name(this._owner);
        this._dbus.unexport();
        this._dbus = null;
        this._service = null;
    }
}
