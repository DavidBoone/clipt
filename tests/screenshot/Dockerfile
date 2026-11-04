# The README screenshot: real GNOME Settings under headless GNOME Shell, on
# top of the GNOME test image.
FROM clipt-gnome
RUN pacman -Sy --noconfirm --needed gnome-control-center gnome-backgrounds adwaita-icon-theme cantarell-fonts >/dev/null && pacman -Scc --noconfirm >/dev/null
# A normal user: as root GNOME Shell shows a "privileged user" notification.
RUN useradd -m -u 1000 sam && ln -sf /clipt /usr/local/bin/clipt
