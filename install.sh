#!/bin/bash
set -e

uninstall() {
    echo "Uninstalling cosmic-monitor-hack..."

    echo "Stopping and disabling service..."
    systemctl --user stop cosmic-monitor-hack.service || true
    systemctl --user disable cosmic-monitor-hack.service || true

    echo "Removing systemd service file..."
    rm -f ~/.config/systemd/user/cosmic-monitor-hack.service
    systemctl --user daemon-reload

    echo "Removing binary..."
    rm -f ~/.local/bin/cosmic-monitor-hack

    echo "Uninstallation complete!"
    echo ""
    echo "Note: Configuration files in ~/.config/cosmic-monitor-hack/ were not removed."
    echo "Remove them manually if desired."
    exit 0
}

# Check for --uninstall flag
if [ "$1" = "--uninstall" ]; then
    uninstall
fi

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
echo ""
echo "To uninstall:"
echo "  ./install.sh --uninstall"
