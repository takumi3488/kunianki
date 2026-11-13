#!/bin/bash
set -euo pipefail

# Run only in remote environment
if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

# Slint build dependencies plus the runtime libraries the slint-lsp release binary needs
sudo apt-get update
sudo apt-get install -y build-essential libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libfontconfig-dev libinput10 libgbm1

# slint-lsp (formatter) matching the app's locked slint version
SLINT_VERSION=$(awk '/^name = "slint"$/ { getline; gsub(/"/, "", $3); print $3; exit }' Cargo.lock)
case "$(uname -m)" in
  x86_64)
    SLINT_LSP_ARCHIVE=slint-lsp-x86_64-unknown-linux-gnu.tar.gz
    SLINT_LSP_BINARY=slint-lsp/slint-lsp
    ;;
  aarch64)
    SLINT_LSP_ARCHIVE=slint-lsp-aarch64-unknown-linux-gnu.tar.gz
    SLINT_LSP_BINARY=slint-lsp-aarch64-unknown-linux-gnu/slint-lsp
    ;;
  *) echo "No slint-lsp release binary for $(uname -m)" >&2; exit 1 ;;
esac

TMP_DIR=$(mktemp -d)
trap 'rm -rf "$TMP_DIR"' EXIT
curl -fsSL "https://github.com/slint-ui/slint/releases/download/v${SLINT_VERSION}/${SLINT_LSP_ARCHIVE}" | tar -xz -C "$TMP_DIR"
install -m 755 "$TMP_DIR/$SLINT_LSP_BINARY" "$HOME/.cargo/bin/slint-lsp"

if ! command -v taplo >/dev/null 2>&1; then
  cargo install taplo-cli --locked
fi

cargo build
exit 0
