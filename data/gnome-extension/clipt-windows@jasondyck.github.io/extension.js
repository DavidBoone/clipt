// clipt's window list for GNOME. GNOME Shell doesn't tell applications where
// windows are (its own Introspect API answers only the screen-sharing portal,
// and gives sizes, not positions), so clipt's overlay can only highlight
// whole monitors there. This extension answers one question for clipt: the
// visible windows on the current workspace, topmost first, as Mutter's frame
// rectangles in its own layout coordinates. clipt maps those onto its
// monitors with org.gnome.Mutter.DisplayConfig.
//
// Window titles are what GNOME keeps from applications, so it answers only a
// caller whose program is named clipt. That stops casual snooping, not a
// determined program on your session.

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
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
  </interface>
</node>`;

const KINDS = [Meta.WindowType.NORMAL, Meta.WindowType.DIALOG, Meta.WindowType.MODAL_DIALOG, Meta.WindowType.UTILITY];

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

class Service {
    async ListAsync(_params, invocation) {
        let allowed = false;
        try {
            allowed = await isClipt(invocation);
        } catch (e) {
            logError(e, 'clipt window list');
        }
        if (!allowed) {
            invocation.return_dbus_error('org.freedesktop.DBus.Error.AccessDenied', 'only clipt may list windows');
            return;
        }
        invocation.return_value(new GLib.Variant('(a(iiiisss))', [visibleWindows()]));
    }
}

export default class CliptWindows extends Extension {
    enable() {
        this._dbus = Gio.DBusExportedObject.wrapJSObject(IFACE, new Service());
        this._dbus.export(Gio.DBus.session, PATH);
        this._owner = Gio.bus_own_name_on_connection(Gio.DBus.session, NAME, Gio.BusNameOwnerFlags.NONE, null, null);
    }

    disable() {
        Gio.bus_unown_name(this._owner);
        this._dbus.unexport();
        this._dbus = null;
    }
}
