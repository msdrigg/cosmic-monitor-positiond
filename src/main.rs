#![allow(clippy::single_match)]

use calloop::{EventLoop, LoopHandle};
use calloop_wayland_source::WaylandSource;
use std::{process::Command, time::Duration};
use wayland_client::{
    delegate_noop,
    globals::{registry_queue_init, GlobalListContents},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1, ext_idle_notifier_v1,
};
use wayland_client::protocol::{wl_registry, wl_seat};

const IDLE_TIMEOUT_MS: u32 = 5 * 60 * 1000; // 5 minutes in milliseconds
const MAX_MONITOR_DETECTION_ATTEMPTS: u32 = 10;
const MONITOR_DETECTION_INTERVAL: Duration = Duration::from_secs(1);

struct IdleNotification {
    notification: ext_idle_notification_v1::ExtIdleNotificationV1,
}

impl IdleNotification {
    fn new(
        idle_notifier: &ext_idle_notifier_v1::ExtIdleNotifierV1,
        seat: &wl_seat::WlSeat,
        timeout_ms: u32,
        qh: &QueueHandle<State>,
    ) -> Self {
        let notification = idle_notifier.get_idle_notification(timeout_ms, seat, qh, ());
        Self { notification }
    }
}

impl Drop for IdleNotification {
    fn drop(&mut self) {
        self.notification.destroy();
    }
}

struct State {
    idle_notifier: ext_idle_notifier_v1::ExtIdleNotifierV1,
    seat: wl_seat::WlSeat,
    idle_notification: Option<IdleNotification>,
    loop_handle: LoopHandle<'static, Self>,
}

impl State {
    fn handle_idle(&mut self) {
        println!("idling!");
    }

    fn handle_resume(&mut self) {
        println!("resuming!");

        // Spawn monitor positioning in a separate thread to not block the event loop
        std::thread::spawn(|| {
            position_monitors();
        });
    }

    fn recreate_notification(&mut self, qh: &QueueHandle<State>) {
        self.idle_notification = Some(IdleNotification::new(
            &self.idle_notifier,
            &self.seat,
            IDLE_TIMEOUT_MS,
            qh,
        ));
    }
}

fn position_monitors() {
    for attempt in 1..=MAX_MONITOR_DETECTION_ATTEMPTS {
        println!("Checking for monitors (attempt {}/{})", attempt, MAX_MONITOR_DETECTION_ATTEMPTS);

        // Get list of connected outputs in KDL format
        let output = match Command::new("cosmic-randr")
            .args(["list", "--kdl"])
            .output()
        {
            Ok(output) => output,
            Err(err) => {
                eprintln!("Failed to execute cosmic-randr: {}", err);
                std::thread::sleep(MONITOR_DETECTION_INTERVAL);
                continue;
            }
        };

        let outputs_str = match std::str::from_utf8(&output.stdout) {
            Ok(s) => s,
            Err(err) => {
                eprintln!("Failed to parse cosmic-randr output: {}", err);
                std::thread::sleep(MONITOR_DETECTION_INTERVAL);
                continue;
            }
        };

        // Check if both monitors are connected and enabled
        let has_hdmi = outputs_str.contains("output \"HDMI-A-1\" enabled=#true");
        let has_dp = outputs_str.contains("output \"DP-1\" enabled=#true");

        if has_hdmi && has_dp {
            println!("Both monitors detected. Applying configuration...");

            // Apply monitor positions
            if let Err(err) = Command::new("cosmic-randr")
                .args(["position", "HDMI-A-1", "0", "0"])
                .status()
            {
                eprintln!("Failed to position HDMI-A-1: {}", err);
                std::thread::sleep(MONITOR_DETECTION_INTERVAL);
                continue;
            }

            if let Err(err) = Command::new("cosmic-randr")
                .args(["position", "DP-1", "2560", "0"])
                .status()
            {
                eprintln!("Failed to position DP-1: {}", err);
                std::thread::sleep(MONITOR_DETECTION_INTERVAL);
                continue;
            }

            println!("Monitor configuration applied successfully.");
            return;
        }

        if attempt < MAX_MONITOR_DETECTION_ATTEMPTS {
            println!("Waiting for both monitors to be detected...");
            std::thread::sleep(MONITOR_DETECTION_INTERVAL);
        }
    }

    eprintln!("Timeout: Not all monitors detected after {} seconds.", MAX_MONITOR_DETECTION_ATTEMPTS);
}

fn main() {
    env_logger::init();

    println!("Starting cosmic-monitor-hack daemon...");

    // Run monitor positioning on startup
    println!("Running initial monitor positioning check...");
    position_monitors();

    // Setup Wayland connection
    let connection = Connection::connect_to_env().unwrap();
    let (globals, event_queue) = registry_queue_init::<State>(&connection).unwrap();
    let qh = event_queue.handle();

    // Bind to required Wayland protocols
    let idle_notifier = globals
        .bind::<ext_idle_notifier_v1::ExtIdleNotifierV1, _, _>(&qh, 1..=1, ())
        .expect("ext-idle-notifier-v1 not available");

    let seat = globals
        .bind::<wl_seat::WlSeat, _, _>(&qh, 1..=1, ())
        .expect("wl_seat not available");

    // Create event loop
    let mut event_loop: EventLoop<State> = EventLoop::try_new().unwrap();

    let mut state = State {
        idle_notifier,
        seat,
        idle_notification: None,
        loop_handle: event_loop.handle(),
    };

    // Create initial idle notification
    state.recreate_notification(&qh);

    // Setup Wayland event source
    WaylandSource::new(connection, event_queue)
        .insert(event_loop.handle())
        .unwrap();

    println!("Monitoring for idle/resume events...");

    // Run event loop
    loop {
        if event_loop.dispatch(None, &mut state).is_err() {
            break;
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _state: &mut Self,
        _: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // We don't need to handle registry events for this simple use case
    }
}

impl Dispatch<ext_idle_notification_v1::ExtIdleNotificationV1, ()> for State {
    fn event(
        state: &mut Self,
        _notification: &ext_idle_notification_v1::ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => {
                state.handle_idle();
            }
            ext_idle_notification_v1::Event::Resumed => {
                state.handle_resume();
            }
            _ => {}
        }
    }
}

delegate_noop!(State: ignore wl_seat::WlSeat);
delegate_noop!(State: ext_idle_notifier_v1::ExtIdleNotifierV1);
