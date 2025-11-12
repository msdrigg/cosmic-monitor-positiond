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

echo "Creating default config file if it doesn't exist..."
mkdir -p ~/.config/cosmic-monitor-positiond
if [ ! -f ~/.config/cosmic-monitor-positiond/state.toml ]; then
    cat > ~/.config/cosmic-monitor-positiond/state.toml << 'EOF'
# COSMIC Monitor Configuration
#
# This file stores your monitor configurations in TOML format.
# Each monitor is defined as a table with various display properties.
#
# All fields are optional. Leave them out or set to null to use automatic values.
# Run 'cosmic-monitor-positiond save' to save your current monitor configuration.
#
# Format:
# [montiors]
# [monitors.<MonitorId>]
# serial_number = "string"              # Monitor serial number
# pos = [x, y]                          # Position in pixels
# size = [width, height]                # Resolution in pixels
# refresh = millihertz                  # Refresh rate in millihertz (e.g., 59951 for ~60Hz)
# adaptive_sync = "mode"                # "Disabled", "Automatic", or "Always"
# scale = float                         # Display scaling factor (e.g., 1.0, 1.5, 2.0)
# transform = "orientation"             # "normal", "90", "180", "270", "flipped", etc.
# primary = boolean                     # Set as primary display for X11 apps
#
# Example:
# [monitors]
#
# [monitors.HDMI-1]
# serial_number = "123123031"
# pos = [0, 0]
# size = [2560, 1440]
# refresh = 59951
# adaptive_sync = "Automatic"
# scale = 1.0
# transform = "normal"
# primary = true
#
# [monitors.DP-1]
# pos = [2560, 0]

[monitors]

EOF
    echo "Created default config file at ~/.config/cosmic-monitor-positiond/state.toml"
fi

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
