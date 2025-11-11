#!/bin/bash
set -e

echo "Building cosmic-monitor-hack..."
cargo build --release

echo "Installing binary..."
mkdir -p ~/.local/bin
cp target/release/cosmic-monitor-hack ~/.local/bin/cosmic-monitor-hack
chmod +x ~/.local/bin/cosmic-monitor-hack

echo "Installing systemd service..."
mkdir -p ~/.config/systemd/user
cp cosmic-monitor-hack.service ~/.config/systemd/user/cosmic-monitor-hack.service

echo "Enabling and starting service..."
systemctl --user daemon-reload
systemctl --user enable cosmic-monitor-hack.service
systemctl --user restart cosmic-monitor-hack.service

echo "Installation complete!"
echo "Check status with: systemctl --user status cosmic-monitor-hack.service"
echo "View logs with: journalctl --user -u cosmic-monitor-hack.service -f"
