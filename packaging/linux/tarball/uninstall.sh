#!/usr/bin/env bash
# Remove a tarball install of TrontSnap (the counterpart of install.sh).
#
#   ./uninstall.sh                          remove it, plus the autostart entry and
#                                           GNOME keybindings it created
#   ./uninstall.sh --keep-desktop-settings  leave autostart + keybindings alone
#   PREFIX=/opt/ts ./uninstall.sh           if you installed with a custom PREFIX
#
# Never touches your captures (Pictures/TrontSnap) or ~/.config/trontsnap.
set -euo pipefail

PREFIX="${PREFIX:-$HOME/.local}"
BIN="$PREFIX/bin/trontsnap"
APP_ID=xyz.tront.TrontSnap
KEEP_DESKTOP=0
[[ ${1:-} == --keep-desktop-settings ]] && KEEP_DESKTOP=1

SCHEMA=org.gnome.settings-daemon.plugins.media-keys
KB=/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings

# HARD-WON: leaving the keybindings in place after the binary is gone is worse
# than useless -- TrontSnap also cleared GNOME's own Print binding, so Print would
# do nothing at all. Undo both, but only if the binds point at THIS install (a
# second install, e.g. the .deb, may own them).
undo_gnome() {
    command -v gsettings >/dev/null 2>&1 || return 0
    local cmd cur item slot key keep=()
    cmd="$(gsettings get "$SCHEMA.custom-keybinding:$KB/trontsnap-full/" command 2>/dev/null)" || return 0
    if [[ $cmd != *"$BIN"* ]]; then
        echo "GNOME keybindings point elsewhere ($cmd): left alone."
        return 0
    fi
    cur="$(gsettings get "$SCHEMA" custom-keybindings)"
    cur="${cur#@as }"; cur="${cur#[}"; cur="${cur%]}"
    local IFS=','
    for item in $cur; do
        item="${item//[\' ]/}"
        [[ -z $item || $item == */trontsnap-* ]] || keep+=("'$item'")
    done
    unset IFS
    gsettings set "$SCHEMA" custom-keybindings "[$(IFS=,; printf '%s' "${keep[*]:-}" | sed 's/,/, /g')]"
    for slot in full region record ocr; do
        for key in name command binding; do
            gsettings reset "$SCHEMA.custom-keybinding:$KB/trontsnap-$slot/" "$key" 2>/dev/null || true
        done
    done
    echo "Removed the TrontSnap GNOME keybindings."
    case "$(gsettings get org.gnome.shell.keybindings show-screenshot-ui 2>/dev/null || true)" in
        "[]"|"@as []")
            gsettings reset org.gnome.shell.keybindings show-screenshot-ui || true
            echo "Gave the Print key back to GNOME's screenshot UI."
            ;;
    esac
}

if ((!KEEP_DESKTOP)); then
    undo_gnome
    autostart="${XDG_CONFIG_HOME:-$HOME/.config}/autostart/trontsnap.desktop"
    if [[ -f $autostart ]] && grep -qF "$BIN" "$autostart"; then
        rm -f "$autostart"
        echo "Removed the autostart entry."
    fi
fi

rm -f "$BIN" "$PREFIX/share/applications/$APP_ID.desktop"
find "$PREFIX/share/icons/hicolor" -path '*/apps/trontsnap.png' -delete 2>/dev/null || true
rm -rf "$PREFIX/share/doc/trontsnap" "$PREFIX/share/trontsnap/uninstall.sh"
rmdir "$PREFIX/share/trontsnap" 2>/dev/null || true
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q "$PREFIX/share/applications" 2>/dev/null || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f "$PREFIX/share/icons/hicolor" 2>/dev/null || true
fi
echo "Uninstalled TrontSnap from $PREFIX. Your captures and settings were not touched."
