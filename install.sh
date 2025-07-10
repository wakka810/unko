#!/bin/bash

set -e

echo "Building unko in release mode..."
cargo build --release

INSTALL_PATH="/usr/local/bin/unko"
echo "Installing unko to $INSTALL_PATH..."
sudo cp target/release/unko "$INSTALL_PATH"

if ! grep -Fxq "$INSTALL_PATH" /etc/shells; then
  echo "Adding $INSTALL_PATH to /etc/shells..."
  echo "$INSTALL_PATH" | sudo tee -a /etc/shells
fi

echo ""
echo "Installation complete!"
echo "You can now set unko as your default shell by running:"
echo "  chsh -s $INSTALL_PATH"
