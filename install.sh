#!/bin/sh
# Installs sylva for the current user: the binary on PATH, plus the desktop
# entry and icon that make it a launchable application.
#
# Everything goes under $HOME, so no root is needed and nothing outside the
# user's own directories is touched.
#
#   ./install.sh                            build and install
#   ./install.sh --uninstall                remove what this script installed
#   ./install.sh --print-desktop-entry DIR  render the entry for DIR and exit
#
set -eu

BIN_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
APP_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
ICON_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/scalable/apps"

BINARY="$BIN_DIR/sylva"
DESKTOP="$APP_DIR/sylva.desktop"
ICON="$ICON_DIR/sylva.svg"

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

refresh_desktop_database() {
    # Best effort: the entry works without it, it just may take a session
    # restart to appear.
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$APP_DIR" 2>/dev/null || true
    fi
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -qtf "${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor" 2>/dev/null || true
    fi
}

# Writes the packaged entry to stdout with Exec pointing at $1/sylva.
#
# The template names the binary bare, which only works when the *session* PATH
# has $BIN_DIR on it. A GNOME Wayland session never reads ~/.profile, so on many
# machines it does not, and GIO drops an entry whose Exec it cannot resolve
# outright: the launcher does not fail to start, it never appears in the menu at
# all. Naming the binary in full removes the dependency on the session PATH.
render_desktop_entry() {
    binary="$1/sylva"

    case $binary in
        *[!A-Za-z0-9/._-]*)
            # The spec wants " and \ backslash-escaped inside the quotes.
            escaped=$(printf '%s' "$binary" | sed 's/[\\"]/\\&/g')
            exec_value="\"$escaped\""
            ;;
        *) exec_value=$binary ;;
    esac

    # Passed through the environment rather than awk -v, which would eat the
    # backslashes the escaping above just added.
    EXEC_VALUE="$exec_value" awk '
        /^Exec=/ { print "Exec=" ENVIRON["EXEC_VALUE"] " --welcome %f"; next }
        { print }
    ' "$here/packaging/sylva.desktop"
}

# Anything unrecognised stops here. Falling through to a full build and install
# is far too surprising a thing to do with an argument nobody asked for.
case "${1:-}" in
    "") ;;
    --uninstall)
        rm -f "$BINARY" "$DESKTOP" "$ICON"
        refresh_desktop_database
        echo "sylva: removed $BINARY, $DESKTOP and $ICON"
        exit 0
        ;;
    --print-desktop-entry)
        if [ $# -ne 2 ]; then
            echo "usage: $0 --print-desktop-entry <bin-dir>" >&2
            exit 2
        fi
        render_desktop_entry "$2"
        exit 0
        ;;
    *)
        echo "$0: unknown option $1" >&2
        exit 2
        ;;
esac

# rustup installs here and asks the shell profile to add it, which a
# non-interactive shell never reads. Look for it directly before giving up.
if ! command -v cargo >/dev/null 2>&1 && [ -x "${CARGO_HOME:-$HOME/.cargo}/bin/cargo" ]; then
    PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
    export PATH
fi

if ! command -v cargo >/dev/null 2>&1; then
    echo "sylva: cargo is not on PATH. Install a Rust toolchain first: https://rustup.rs" >&2
    exit 1
fi

echo "sylva: building (this takes a few minutes the first time)"
cargo build --release --manifest-path "$here/Cargo.toml"

mkdir -p "$BIN_DIR" "$APP_DIR" "$ICON_DIR"

# Copied rather than symlinked: a symlink into target/ breaks the moment
# `cargo clean` runs, and does so silently.
install -m 755 "$here/target/release/sylva" "$BINARY"
render_desktop_entry "$BIN_DIR" > "$DESKTOP"
chmod 644 "$DESKTOP"
install -m 644 "$here/packaging/sylva.svg" "$ICON"

refresh_desktop_database

echo "sylva: installed $BINARY"
echo "sylva: installed $DESKTOP"

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
        echo
        echo "sylva: $BIN_DIR is not on your PATH. Add this to your shell profile:"
        echo "    export PATH=\"$BIN_DIR:\$PATH\""
        ;;
esac

echo
echo "You can now:"
echo "  * run 'sylva' inside any repository"
echo "  * launch sylva from the applications menu"
echo "  * right-click a folder in Files and choose Open With > sylva"
