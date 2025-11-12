# Cosmic Monitor Positiond

Does cosmic-de keep forgetting your saved monitor position? Do you constantly have to re-position your monitor screens half of the time when your computer wakes from idle? Then this repo is for you.

This repository will save your current monitor positioning and then re-create that positioning after your computer idles and wakes up.

## Installation

**Requirements**
- Pop-OS Cosmic DE (to run the project)
- rust + cargo toolchain installed (to build the project)
- libudev-dev installed (for building with support for auto-detecting monitors)

1. Clone the repo
2. Run `./install.sh` (or `./install.sh --no-autodetect` to not support monitor autodetect)

This will build the project and install a user-level systemctl file that runs on login to keep our idle-monitor alive. This idle monitor will detect idle or screen changes and then try to re-position them according to your saved configuration. It will also install the cosmic-monitor-positiond binary in ~/.local/bin

## Usage

**Changing your saved position**

- If you want to change your monitor arangement, you can manually set your arangement in system settings, and then run `~/.local/bin/cosmic-monitor-positiond save` to have cosmic-monitor-positiond save your changes

**Uninsatlling**

1. Clone the repo again
2. Run `./install.sh --uninsatll`
