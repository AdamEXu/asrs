#!/bin/sh
# Adds asrs to this user's desktop: a menu entry, an icon and the log file types, with asrs
# offered for them but never as their default. Run it again after moving this folder.
# --uninstall takes them back out (defaults pinned below stay: they were the defaults before).
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
data=${XDG_DATA_HOME:-$HOME/.local/share}
desktop=$data/applications/asrs.desktop
mime=$data/mime/packages/net.adamxu.asrs.xml
sizes="16 32 48 64 128 256 512"
types="@MIME_LIST@"

refresh() {
    if command -v update-mime-database >/dev/null 2>&1; then
        update-mime-database "$data/mime" >/dev/null 2>&1 || true
    fi
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$data/applications" >/dev/null 2>&1 || true
    fi
}

if [ "${1:-}" = --uninstall ]; then
    for size in $sizes; do
        unlink "$data/icons/hicolor/${size}x$size/apps/asrs.png" 2>/dev/null || true
    done
    unlink "$desktop" 2>/dev/null || true
    unlink "$mime" 2>/dev/null || true
    refresh
    exit 0
fi

# GLib looks for a program named in Exec before it expands %%, so no escape works for both.
case $here in
*%*)
    echo "asrs cannot be added to the menu from a folder with % in its path: $here" >&2
    exit 1
    ;;
esac

mkdir -p "$data/applications" "$data/mime/packages"
for size in $sizes; do
    icons=$data/icons/hicolor/${size}x$size/apps
    mkdir -p "$icons"
    cp "$here/share/icons/hicolor/${size}x$size/apps/asrs.png" "$icons/asrs.png"
done
cp "$here/share/mime/packages/net.adamxu.asrs.xml" "$mime"
refresh

# An application newly listed for a type can become its default when the user never chose one,
# so each type's handler from before the entry exists is made its explicit default.
pinned=
if command -v xdg-mime >/dev/null 2>&1; then
    for type in $types; do
        handler=$(xdg-mime query default "$type" 2>/dev/null || true)
        if [ -n "$handler" ] && [ "$handler" != asrs.desktop ]; then
            pinned="$pinned $type=$handler"
        fi
    done
fi

# Exec is unescaped twice, as a string and then as a quoted argument. The last expression is for
# the replacement below.
exe=$(printf '%s' "$here/asrs" |
    sed 's/\\/\\\\\\\\/g; s/["`$]/\\\\&/g; s/[\\&|]/\\&/g')
sed "s|^Exec=.*|Exec=\"$exe\" %F|" "$here/share/applications/asrs.desktop" >"$desktop"
refresh
for pin in $pinned; do
    xdg-mime default "${pin#*=}" "${pin%%=*}" || true
done
