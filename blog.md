---
slug: cosmic-de-wayland-monitor-positiond
title: Using
authors: msdrigg
tags:
    - linux
draft: true
---

### Background: The Goal

-   **Problem**: Need to run custom scripts at two specific times:
    -   After user login (for session initialization)
    -   After system wakes from suspend (for monitor reconfiguration)
-   **Environment**: COSMIC Desktop Environment with systemd
-   **Initial uncertainty**: How to hook into cosmic-greeter's login process

### Investigation: How COSMIC Greeter Handles Sessions

-   **Explored the cosmic-greeter codebase**
    -   Found that cosmic-greeter is a thin UI client for `greetd`
    -   Key finding: Greeter exits immediately after authentication succeeds
    -   No built-in post-login hook system exists in cosmic-greeter
    -   Session management delegated entirely to greetd
-   **Identified available integration points**
    -   PAM session modules (runs as root, complex)
    -   Desktop entry wrappers (requires modifying system files)
    -   Shell profile files (only works for login shells)
    -   **systemd user units** (✓ recommended approach)

### Solution Part 1: Post-Login Service

-   **Created systemd user service for post-login**
    -   Target: `graphical-session.target` (runs when graphical session starts)
    -   Service location: `~/.config/systemd/user/post-login.service`
    -   Configuration:
        -   `Type=oneshot` - runs once and completes
        -   `RemainAfterExit=yes` - keeps service marked as active
        -   `WantedBy=graphical-session.target` - automatically starts with graphical session
    -   Commands to enable:
        ```bash
        systemctl --user enable post-login.service
        systemctl --user start post-login.service
        ```

### Solution Part 2: Post-Suspend/Wake Service (Initial Attempt)

-   **Referenced systemd.special documentation**

    -   Found `sleep.target` as the correct target for suspend/wake hooks
    -   Key insight: Use `ExecStop=` for post-wake scripts (not `ExecStart=`)
    -   Pattern explained:
        -   `ExecStart` runs BEFORE sleep
        -   `ExecStop` runs AFTER waking up
        -   `RemainAfterExit=yes` keeps service active during sleep
        -   `StopWhenUnneeded=yes` stops service after wake completes

-   **Created initial user service**
    -   Target: `sleep.target`
    -   Service location: `~/.config/systemd/user/cosmic-monitor-setup-suspend.service`
    -   Initial configuration worked for user context

### Conversion: Moving from User to System Service

-   **Reason for conversion**: Needed system-level execution
-   **Key changes required**:
    -   Move from `~/.config/systemd/user/` to `/etc/systemd/system/`
    -   Change from `systemctl --user` to `sudo systemctl`
    -   Add `User=scottdriggers` to run as specific user
    -   Replace `%h` with absolute path `/home/scottdriggers/...`
-   **Migration steps**:
    1. Disable and remove old user service
    2. Create new system service file
    3. Reload system daemon
    4. Enable new system service

### Problem: Wayland Connection Errors

-   **Observed symptoms**:
    Error: WaylandConnection(NoCompositor)

-   `cosmic-randr` couldn't connect to Wayland compositor
-   Script failed repeatedly despite correct user

-   **Root cause**:
-   System service lacks user session environment variables
-   Required variables for Wayland connection:
    -   `WAYLAND_DISPLAY` - specifies which Wayland socket to use
    -   `XDG_RUNTIME_DIR` - directory for user's runtime files
    -   `DBUS_SESSION_BUS_ADDRESS` - D-Bus session bus location

### Solution: Adding Environment Variables to System Service

-   **Diagnosed missing variables**:

```bash
# Commands to identify needed values
id -u scottdriggers  # Get UID (typically 1000)
echo $WAYLAND_DISPLAY  # Get Wayland display (typically wayland-1)
echo $XDG_RUNTIME_DIR  # Get runtime dir (typically /run/user/1000)
```

-   Updated service configuration

```
[Service]
Type=oneshot
RemainAfterExit=yes
User=scottdriggers
Environment="XDG_RUNTIME_DIR=/run/user/1000"
Environment="WAYLAND_DISPLAY=wayland-1"
Environment="DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus"
ExecStart=/bin/true
ExecStop=/home/scottdriggers/.local/bin/cosmic-monitor-setup.sh
Applied and tested:
```

-   Tested (Success)

```
sudo systemctl daemon-reload
sudo systemctl restart cosmic-monitor-setup-suspend.service
systemctl status cosmic-monitor-setup-suspend.service

Nov 10 10:36:21 pop-os systemd[3953]: Starting cosmic-monitor-setup.service - COSMIC Monitor Positioning Setup>
Nov 10 10:36:21 pop-os cosmic-monitor-setup.sh[656674]: Both monitors detected. Applying configuration...
Nov 10 10:36:21 pop-os cosmic-monitor-setup.sh[656674]: Monitor configuration applied successfully.
Nov 10 10:36:21 pop-os systemd[3953]: Finished cosmic-monitor-setup.service - COSMIC Monitor Positioning Setup.
```

### Final Architecture

-   Post-login service (user-level):
    -   Location: ~/.config/systemd/user/
    -   Target: graphical-session.target
    -   Runs in user context with full environment
-   Post-suspend service (system-level):
    -   Location: /etc/systemd/system/
    -   Target: sleep.target
    -   Runs as specified user with injected environment variables
    -   Triggers on all sleep events (suspend, hibernate, hybrid-sleep)

### Lessons Learned

-   systemd targets provide clean integration points
    -   No need to modify display manager or login system
    -   Works across different display managers (GDM, SDDM, cosmic-greeter)
-   System vs user services trade-offs:
    -   User services inherit session environment automatically
    -   System services require explicit environment configuration
    -   System services run earlier in boot/wake cycle
-   Wayland applications need specific environment:
    -   WAYLAND_DISPLAY is not optional
    -   XDG_RUNTIME_DIR must point to user's runtime directory
    -   D-Bus session address needed for IPC
-   systemd service lifecycle is powerful:
    -   ExecStop= pattern for post-wake scripts is elegant
    -   RemainAfterExit=yes keeps service active across sleep
    -   Proper use of dependencies ensures correct ordering

### Debugging Tips

-   Check service status: systemctl status <service>
-   View logs: journalctl -u <service> -n 50
-   Test immediately: Start/stop services without waiting for login/suspend
-   Verify environment: Echo variables from scripts to logs for debugging

### AFTER_FIX_TEST_1 >>>

[Unit]
Description=Reload thinkfan after waking up from suspend
After=sysinit.target
After=suspend.target
After=suspend-then-hibernate.target
After=hybrid-sleep.target
After=hibernate.target

[Service]
Type=oneshot
ExecStart=/usr/bin/pkill -x -usr2 thinkfan

[Install]
WantedBy=sleep.target

### AFTER_FIX_TEST_FORREAL

-   So sleep is aparently never triggered on my system because I guess I do not have suspend enabled (I have a desktop), I just have screen shutdown enabled
-   So I need to figure out how cosmic handles screen off notifications
-   Found cosmic-idle which does idle detection wakeup and found their wayland-protocls that they use to manage it
-   Built out my own version of cosmic idle that detects wakeup and idle and does the same thing in rust
-   Built simple user installer and set it up
-   Tested and https://github.com/msdrigg/cosmic-monitor-hack working

-   Added save + apply + monitor configuration and ready to go
