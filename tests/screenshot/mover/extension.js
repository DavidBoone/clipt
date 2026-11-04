import GLib from 'gi://GLib';

// Puts new windows where ~/.places says ("wm_class x y w h" per line):
// without unsafe mode, GNOME Shell offers no other way to place a window.
export default class Mover {
  enable() {
    this.id = global.display.connect('window-created', (_d, win) => {
      const place = () => {
        const [, data] = GLib.file_get_contents(GLib.get_home_dir() + '/.places');
        for (const line of new TextDecoder().decode(data).split('\n')) {
          const [cls, x, y, w, h] = line.split(' ');
          if (cls && (win.get_wm_class() ?? '').toLowerCase().includes(cls))
            win.move_resize_frame(true, +x, +y, +w, +h);
        }
        return GLib.SOURCE_REMOVE;
      };
      // Once it has mapped, and again after the app has sized itself.
      GLib.timeout_add(GLib.PRIORITY_DEFAULT, 400, place);
      GLib.timeout_add(GLib.PRIORITY_DEFAULT, 1200, place);
    });
  }
  disable() { global.display.disconnect(this.id); }
}
