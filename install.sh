#!/bin/bash
set -e

echo "Building cosmic-monitor-hack..."
cargo build --release

echo "Stopping existing service if it exists..."
systemctl --user stop cosmic-monitor-hack.service || true

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
echo ""
echo "Usage:"
echo "  cosmic-monitor-hack save     - Save current monitor configuration"
echo "  cosmic-monitor-hack apply    - Apply saved configuration once"
echo "  cosmic-monitor-hack monitor  - Monitor for idle/resume (service mode)"
echo ""
echo "Service commands:"
echo "  systemctl --user status cosmic-monitor-hack.service"
echo "  journalctl --user -u cosmic-monitor-hack.service -f"
