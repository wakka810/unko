#!/bin/bash
set -e

INSTALL_PATH="/usr/local/bin/unko"

if ! command -v cargo >/dev/null 2>&1; then
  echo "Error: cargo is not installed. Please install Rust first." >&2
  exit 1
fi

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT
cd "$TMPDIR"

git clone https://github.com/wakka810/unko.git
cd unko
echo "Building unko in release mode..."
cargo build --release

if [ -f "$INSTALL_PATH" ]; then
  read -p "unko is already installed. Overwrite? (y/N): " confirm
  if [[ "$confirm" != "y" ]]; then
    echo "Aborting."
    exit 1
  fi
fi

echo "Installing unko to $INSTALL_PATH..."
sudo cp target/release/unko "$INSTALL_PATH"

if ! grep -Fxq "$INSTALL_PATH" /etc/shells; then
  echo "$INSTALL_PATH" | sudo tee -a /etc/shells > /dev/null
fi
