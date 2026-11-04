#!/bin/bash
# Inside the container: GNOME Settings on Displays, clipt's editor over it,
# annotated through Mutter's remote-desktop input, captured to /out.
set -u
if [ "$(id -u)" = 0 ]; then
  dbus-uuidgen --ensure; mkdir -p /run/dbus; dbus-daemon --system --fork
  python3 -m dbusmock --system --template logind >/tmp/mock.log 2>&1 &
  sleep 1
  cp /clipt.desktop /usr/share/applications/
  mkdir -p /run/user/1000 && chown sam /run/user/1000 && chmod 700 /run/user/1000
  exec runuser -u sam -- env HOME=/home/sam XDG_RUNTIME_DIR=/run/user/1000 XDG_CURRENT_DESKTOP=GNOME \
    XDG_SESSION_TYPE=wayland dbus-run-session "$0"
fi

export GSETTINGS_BACKEND=keyfile
dbus-update-activation-environment GSETTINGS_BACKEND=keyfile   # for the portals, which read the dark style
gsettings set org.gnome.desktop.interface color-scheme prefer-dark
gsettings set org.gnome.desktop.background picture-uri-dark file:///usr/share/backgrounds/gnome/blobs-d.svg
gsettings set org.gnome.desktop.interface enable-hot-corners false    # the input driver parks the pointer at 0,0
gsettings set org.gnome.desktop.notifications show-banners false
ext=$HOME/.local/share/gnome-shell/extensions/mover@clipt-screenshot
mkdir -p $ext && cp /t/mover/* $ext/
gsettings set org.gnome.shell enabled-extensions "['mover@clipt-screenshot']"
gnome-shell --headless --wayland --no-x11 --virtual-monitor 1600x1000 >/tmp/gs.log 2>&1 &
for _ in $(seq 100); do [ -S $XDG_RUNTIME_DIR/wayland-0 ] && break; sleep 0.1; done
sleep 4
export WAYLAND_DISPLAY=wayland-0
gdbus call --session --dest org.freedesktop.impl.portal.PermissionStore --object-path /org/freedesktop/impl/portal/PermissionStore \
  --method org.freedesktop.impl.portal.PermissionStore.SetPermission screenshot true screenshot clipt "['yes']" >/dev/null
# GNOME Shell starts in the overview.
gdbus call --session --dest org.gnome.Shell --object-path /org/gnome/Shell --method org.freedesktop.DBus.Properties.Set \
  org.gnome.Shell OverviewActive "<false>" >/dev/null

echo "settings 300 150 1000 700" > $HOME/.places
gnome-control-center display >/tmp/gcc.log 2>&1 & sleep 5
# The editor first: the remote-desktop session puts a sharing icon in the top bar.
clipt gui 2>/tmp/gui.err & sleep 2
mkfifo /tmp/rin; python3 /rdinput.py < /tmp/rin 2>/tmp/rd.err & exec 3>/tmp/rin
act() { for a in "$@"; do echo "$a" >&3; done; sleep 0.3; }
drag() { act "move $1 $2" down "move $(( ($1+$3)/2 )) $(( ($2+$4)/2 ))" "move $3 $4" up; sleep 0.5; }
sleep 1
drag 260 110 1340 880                       # the selection
act "key v"; sleep 0.5                      # the first key goes to the shell
act "key r"; drag 918 394 990 430           # a rectangle round 125 %
act "key n" "move 912 390" click            # counter 1
act "move 622 489" click                    # counter 2, by Night Light
act "key a" "key 5"; drag 800 610 925 445   # a blue arrow
act "key t" "key 7" "move 700 630" click "type Try 125 % first" "key Return"
act "key v" "move 1330 900"; sleep 1
clipt full -o /tmp/shot.png
magick /tmp/shot.png -crop 1180x910+210+75 +repage /out/screenshot.png
