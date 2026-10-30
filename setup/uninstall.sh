#!/usr/bin/env bash
# Removes Closeup for the current user.
#
#   setup/uninstall.sh            app, desktop entry, icon, extension
#   setup/uninstall.sh --system   also the uinput rule, and leaves the 'input' group (sudo)
#
# Your settings (~/.config/closeup) and saved source (~/.local/state/closeup)
# are kept; delete them yourself if you want a clean slate.
set -euo pipefail

uuid="closeup@closeup.app"
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
system=false

for arg in "$@"; do
    case "$arg" in
        --system) system=true ;;
        -h | --help)
            sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "unknown option: $arg" >&2
            exit 2
            ;;
    esac
done

if command -v gnome-extensions >/dev/null; then
    gnome-extensions disable "$uuid" 2>/dev/null || true
fi
rm -rf "${data_dir:?}/gnome-shell/extensions/$uuid"
rm -f "$HOME/.local/bin/closeup" \
    "$data_dir/applications/closeup.desktop" \
    "$data_dir/icons/hicolor/scalable/apps/closeup.svg"

if $system; then
    sudo rm -f /etc/udev/rules.d/99-closeup-uinput.rules /etc/modules-load.d/closeup-uinput.conf
    sudo udevadm control --reload-rules
    sudo gpasswd -d "$USER" input || true
fi

echo "Closeup removed. Log out and back in to unload the extension."
