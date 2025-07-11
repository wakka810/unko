#!/bin/bash
set -e

INSTALL_PATH="/usr/local/bin/unko"

for cmd in cargo git sudo; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "Error: $cmd is not installed. Please install it first." >&2
    exit 1
  fi
done

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT
cd "$TMPDIR"

echo "Cloning and building unko..."
git clone https://github.com/wakka810/unko.git
cd unko
cargo build --release

echo "Installing unko to $INSTALL_PATH (will overwrite if exists)..."
sudo cp -f target/release/unko "$INSTALL_PATH"

if ! grep -Fxq "$INSTALL_PATH" /etc/shells; then
  echo "Registering $INSTALL_PATH in /etc/shells..."
  echo "$INSTALL_PATH" | sudo tee -a /etc/shells > /dev/null
fi

echo ""
echo "Installation complete!"
echo "You can now set unko as your default shell by running:"
echo "  chsh -s $INSTALL_PATH"
