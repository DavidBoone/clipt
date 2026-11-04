# Wine and MinGW on the X11 test image: cross-links clipt.exe and runs it
# on Xvfb + Openbox (tests/wine.sh).
FROM clipt-x11
RUN pacman -Sy --noconfirm --needed wine mingw-w64-gcc >/dev/null && pacman -Scc --noconfirm >/dev/null
