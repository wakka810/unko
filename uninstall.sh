#!/bin/bash

set -e

INSTALL_PATH="/usr/local/bin/unko"

echo "Uninstalling unko..."

if [ -f "$INSTALL_PATH" ]; then
  echo "Removing $INSTALL_PATH..."
  sudo rm "$INSTALL_PATH"
else
  echo "$INSTALL_PATH was not found. Skipping."
fi

if grep -Fxq "$INSTALL_PATH" /etc/shells; then
  echo "Removing $INSTALL_PATH from /etc/shells..."
  sudo sed -i.bak "\|$INSTALL_PATH|d" /etc/shells
  sudo rm /etc/shells.bak
fi

echo ""
echo "Uninstallation complete!"
