#!/usr/bin/env bash
# Installs Closeup for the current user. Run from a source checkout or an
# unpacked release archive.
#
#   setup/install.sh                 everything; asks before granting keyboard access
#   setup/install.sh --input-group   the same, granting keyboard access without asking
#   setup/install.sh --user          only the per-user part: app, desktop entry, extension
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
uuid="closeup@closeup.app"
bin_dir="$HOME/.local/bin"
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
extensions="$data_dir/gnome-shell/extensions"
user_only=false
grant_input=ask
needs_relogin=false
needs_reboot=false

for arg in "$@"; do
    case "$arg" in
        --user) user_only=true ;;
        --input-group) grant_input=yes ;;
        -h | --help)
            sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "unknown option: $arg" >&2
            exit 2
            ;;
    esac
done

step() { printf '\n\033[1m==> %s\033[0m\n' "$1"; }

install_build_dependencies() {
    if command -v apt-get >/dev/null; then
        sudo apt-get install -y libpipewire-0.3-dev libspa-0.2-dev clang pkg-config
    elif command -v dnf >/dev/null; then
        sudo dnf install -y pipewire-devel clang pkgconf-pkg-config
    elif command -v pacman >/dev/null; then
        sudo pacman -S --needed --noconfirm pipewire clang pkgconf
    else
        echo "Unknown package manager: install the PipeWire development headers, clang and pkg-config yourself."
    fi
}

# Bare-digit zoom reads raw keyboard input, which only the 'input' group
# allows, and that group lets every program you run read keystrokes. Ask
# rather than assume; everything else works without it.
wants_keyboard_access() {
    if id -nG "$USER" 2>/dev/null | tr ' ' '\n' | grep -qx input || getent group input | grep -qw "$USER"; then
        return 0
    fi
    if [ "$grant_input" = yes ]; then
        return 0
    fi
    if [ ! -t 0 ]; then
        echo "Skipping keyboard access (not interactive); rerun with --input-group to grant it."
        return 1
    fi
    echo "Bare-digit zoom (1-9 without Ctrl+Alt) needs you in the 'input' group."
    echo "That group lets ANY program you run read raw keyboard input, passwords included."
    echo "Without it, Ctrl+Alt+1-9 and keys typed into the Stage window still zoom."
    local answer
    read -r -p "Join the 'input' group? [y/N] " answer
    [[ "$answer" =~ ^[Yy]$ ]]
}

install_keyboard_access() {
    sudo install -m 0644 "$root/setup/99-closeup-uinput.rules" /etc/udev/rules.d/99-closeup-uinput.rules
    echo uinput | sudo tee /etc/modules-load.d/closeup-uinput.conf >/dev/null
    sudo modprobe uinput
    sudo udevadm control --reload-rules
    sudo udevadm trigger --name-match=uinput
    if ! id -nG "$USER" | tr ' ' '\n' | grep -qx input && ! getent group input | grep -qw "$USER"; then
        sudo usermod -aG input "$USER"
        if [ -e "/var/lib/systemd/linger/$USER" ]; then
            needs_reboot=true
        else
            needs_relogin=true
        fi
    fi
}

build_or_locate_binary() {
    if [ -x "$root/bin/closeup" ]; then
        echo "$root/bin/closeup"
        return
    fi
    if ! command -v cargo >/dev/null; then
        echo "cargo not found: install Rust from https://rustup.rs, or use a release archive" >&2
        exit 1
    fi
    (cd "$root" && cargo build --release --locked -p closeup >&2)
    echo "$root/target/release/closeup"
}

enable_extension() {
    gsettings set org.gnome.shell disable-user-extensions false
    local enabled
    enabled="$(gsettings get org.gnome.shell enabled-extensions)"
    if [[ "$enabled" != *"'$uuid'"* ]]; then
        if [[ "$enabled" == "@as []" || "$enabled" == "[]" ]]; then
            enabled="['$uuid']"
        else
            enabled="${enabled%]}, '$uuid']"
        fi
        gsettings set org.gnome.shell enabled-extensions "$enabled"
    fi
}

if ! $user_only; then
    step "Build dependencies"
    install_build_dependencies
    step "Keyboard access for bare-digit zoom (input group, uinput)"
    if wants_keyboard_access; then
        install_keyboard_access
    else
        echo "Skipped: use Ctrl+Alt+1-9, or rerun with --input-group later."
    fi
fi

step "Closeup app"
binary="$(build_or_locate_binary)"
install -Dm 0755 "$binary" "$bin_dir/closeup"
install -Dm 0644 "$root/packaging/closeup.desktop" "$data_dir/applications/closeup.desktop"
install -Dm 0644 "$root/packaging/closeup.svg" "$data_dir/icons/hicolor/scalable/apps/closeup.svg"
if command -v update-desktop-database >/dev/null; then
    update-desktop-database "$data_dir/applications" || true
fi

step "GNOME Shell extension"
if [ ! -d "$extensions/$uuid" ] || ! diff -qr "$root/gnome-extension/$uuid" "$extensions/$uuid" >/dev/null; then
    needs_relogin=true
fi
mkdir -p "$extensions"
rm -rf "${extensions:?}/$uuid"
cp -r "$root/gnome-extension/$uuid" "$extensions/"
if command -v gsettings >/dev/null; then
    enable_extension
fi

step "Done"
case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "Add $bin_dir to your PATH to run 'closeup' from a terminal." ;;
esac
if $needs_reboot; then
    echo "Reboot before the first run: lingering keeps your user session's old groups alive across logouts."
elif $needs_relogin; then
    echo "Log out and back in before the first run (GNOME only loads extension code, and new groups, at login)."
fi
echo "Then start Closeup from your app grid, or run 'closeup'. 'closeup --doctor' checks the setup."
