# A clean SteamOS-like system for `just deck-check`: Arch Linux (SteamOS's
# base) with only what SteamOS has that the game uses: ALSA, the X client
# libraries SDL3 loads with dlopen (libX11 and libXext, which its X11
# driver needs; libXcursor, libXi, libXfixes and libXrandr, which it uses
# if there; libxkbcommon for Wayland) and Mesa for its OpenGL renderer;
# systemd's libudev comes with the base. Xvfb stands in for gamescope's
# Xwayland, and xwd and xwininfo look at the screen.
FROM docker.io/library/archlinux:latest
RUN pacman -Syu --noconfirm --needed alsa-lib libx11 libxext libxcursor libxi \
        libxfixes libxrandr libxkbcommon mesa \
        xorg-server-xvfb xorg-xwd xorg-xwininfo \
    && pacman -Scc --noconfirm
