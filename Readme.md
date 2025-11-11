# Cosmic Monitor Hack

Does cosmic-de keep forgetting your saved monitor position? Do you constantly have to re-position your monitor screens half of the time when your computer wakes from idle? Then this repo is for you.

This repository will save your current monitor positioning and then re-create that positioning after your computer idles and wakes up.

## Installation

**Requirements**
- Pop-OS Cosmic DE (to run the project)
- rust + cargo toolchain installed (to build the project)
- libudev-dev installed (for building with support for auto-detecting monitors)

1. Clone the repo
2. Run `./install.sh` (or `./install.sh --no-autodetect` to not support monitor autodetect)

This will build the project and install a user-level systemctl file that runs on login to keep our idle-monitor alive. This idle monitor will detect idle or screen changes and then try to re-position them according to your saved configuration. It will also install the cosmic-monitor-hack binary in ~/.local/bin

## Usage

**Changing your saved position**

- If you get another monitor or you want to change your default setup, you can save it by running `~/.local/bin/cosmic-monitor-hack save`

**Uninsatlling**

1. Clone the repo again
2. Run `./install.sh --uninsatll`
