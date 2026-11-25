#!/usr/bin/env bash
# Runs a command on a private X display that fits a 2200x1300 window, for a Linux machine whose own
# desktop must be left alone. Needs Xvfb, and xfwm4 for what depends on a window manager.
#
#   FONTCONFIG_FILE=<dir>/gray.conf xtask/hosts/linux/display.sh <command>...
set -euo pipefail

exec 9>>"$HOME/.asrs-gui.lock"
flock -n 9 || { echo 'waiting for the other desktop run' >&2; flock 9; }

pids=()
trap 'for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done; wait' EXIT

# Not -displayfd: it skips the display's lock file, and a server that picks its own number then takes
# the socket of a desktop's Xwayland.
display=:90
socket=/tmp/.X11-unix/X${display#:}
lock=/tmp/.X${display#:}-lock
[[ ! -e $socket && ! -L $socket ]] || { echo "$socket exists" >&2; exit 1; }
Xvfb "$display" -screen 0 2880x1800x24 -dpi 96 -nolisten tcp >/dev/null 2>&1 9>&- &
xvfb=$!
pids+=("$xvfb")
# The socket alone may be another server's that took the number since the check above.
until [[ -S $socket && $(tr -d ' ' <"$lock" 2>/dev/null) == "$xvfb" ]]; do
  kill -0 "$xvfb" 2>/dev/null || { echo 'Xvfb did not start' >&2; exit 1; }
  sleep 0.1
done
export DISPLAY=$display XDG_SESSION_TYPE=x11 DBUS_SESSION_BUS_ADDRESS=disabled:
export ELECTRON_OZONE_PLATFORM_HINT=x11
unset WAYLAND_DISPLAY HYPRLAND_INSTANCE_SIGNATURE SWAYSOCK XAUTHORITY
# Chromium takes a device scale from these, and above 1 it draws unhinted text.
unset GDK_SCALE GDK_DPI_SCALE

if command -v xfwm4 >/dev/null; then
  # xfwm4 writes its workspaces to xfconf, which would be the desktop's own configuration.
  config=$HOME/.cache/asrs-display
  mkdir -p "$config"
  env -u DBUS_SESSION_BUS_ADDRESS XDG_CONFIG_HOME="$config" \
    dbus-run-session -- xfwm4 --compositor=off >/dev/null 2>&1 9>&- &
  pids+=("$!")
  for _ in {1..100}; do
    xprop -root _NET_SUPPORTING_WM_CHECK 2>/dev/null | grep -q 'window id' && break
    sleep 0.1
  done
fi

"$@" 9>&-
