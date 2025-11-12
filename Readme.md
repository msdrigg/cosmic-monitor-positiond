# Cosmic Monitor Positiond

Does cosmic-de keep forgetting your saved monitor position? Do you constantly have to re-position your monitor screens when your computer wakes from idle? Then `cosmic-monitor-positiond` is for you.

This repository will save your current monitor positioning and then re-create that positioning after your computer idles and wakes up.

For a writeup on how this got created, check out [my blog](https://scottdriggers.com/blog/cosmic-de-wayland-monitor-positiond)

## Installation

**Requirements**
- Pop-OS Cosmic DE (required to run the project)
- rust + cargo toolchain (required to build the project)
- libudev-dev (required to build with support for auto-detecting monitors)

**Build and Install**
1. Clone the repo
2. Run `./install.sh` (or `./install.sh --no-autodetect` to not support monitor autodetect)

This script will build the project and install the `cosmic-monitor-positiond` binary in `~/.local/bin`. It will then install a user-level systemctl file that runs on login to keep continually monitor for wakeup (and optionally monitor-plugin) events. On each event, it will try to set monitor position and other properties according to your saved configuration.

## Usage

**Changing your saved position**

If you want to change your monitor arangement, you can manually set your arangement in system settings, and then run `~/.local/bin/cosmic-monitor-positiond save` to have cosmic-monitor-positiond save your changes

**Uninstalling**

1. Clone the repo again
2. Run `./install.sh --uninstall`

