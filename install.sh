#!/bin/bash
set -e

uninstall() {
    echo "Uninstalling cosmic-monitor-positiond..."

    echo "Stopping and disabling service..."
    systemctl --user stop cosmic-monitor-positiond.service || true
    systemctl --user disable cosmic-monitor-positiond.service || true

    echo "Removing systemd service file..."
    rm -f ~/.config/systemd/user/cosmic-monitor-positiond.service
    systemctl --user daemon-reload

    echo "Removing binary..."
    rm -f ~/.local/bin/cosmic-monitor-positiond

    echo "Uninstallation complete!"
    echo ""
    echo "Note: Configuration files in ~/.config/cosmic-monitor-positiond/ were not removed."
    echo "Remove them manually if desired."
    exit 0
}

# Check for --uninstall flag
if [ "$1" = "--uninstall" ]; then
    uninstall
fi

echo "Building cosmic-monitor-positiond..."
if [ "$1" = "--no-autodetect" ]; then
    cargo build --release --no-default-features
else
    cargo build --release
fi

echo "Stopping existing service if it exists..."
systemctl --user stop cosmic-monitor-positiond.service || true

echo "Installing binary..."
mkdir -p ~/.local/bin
cp target/release/cosmic-monitor-positiond ~/.local/bin/cosmic-monitor-positiond
chmod +x ~/.local/bin/cosmic-monitor-positiond

echo "Installing systemd service..."
mkdir -p ~/.config/systemd/user
cp cosmic-monitor-positiond.service ~/.config/systemd/user/cosmic-monitor-positiond.service

echo "Enabling and starting service..."
systemctl --user daemon-reload
systemctl --user enable cosmic-monitor-positiond.service
systemctl --user restart cosmic-monitor-positiond.service

echo "Installation complete!"
echo ""
echo "Usage:"
echo "  cosmic-monitor-positiond save     - Save current monitor configuration"
echo "  cosmic-monitor-positiond apply    - Apply saved configuration once"
echo "  cosmic-monitor-positiond monitor  - Monitor for idle/resume (service mode)"
echo ""
echo "Service commands:"
echo "  systemctl --user status cosmic-monitor-positiond.service"
echo "  journalctl --user -u cosmic-monitor-positiond.service -f"
echo ""
echo "To uninstall:"
echo "  ./install.sh --uninstall"
