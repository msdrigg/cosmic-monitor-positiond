#![allow(clippy::single_match)]

use calloop::{EventLoop, LoopHandle};
use calloop_wayland_source::WaylandSource;
use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::{fs, path::PathBuf, process::Command, time::Duration};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{
    delegate_noop,
    globals::{registry_queue_init, GlobalListContents},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1, ext_idle_notifier_v1,
};

const IDLE_TIMEOUT_MS: u32 = 5 * 60 * 1000; // 5 minutes in milliseconds
const MAX_ATTEMPTS: usize = 30;

/// COSMIC monitor positioning daemon
#[derive(Parser, Debug)]
#[command(name = "cosmic-monitor-hack")]
#[command(about = "Automatically position monitors in COSMIC DE")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Save the current monitor configuration
    Save,
    /// Apply the saved monitor configuration once
    Apply,
    /// Monitor for system resume and apply configuration (default mode)
    Monitor,
}

#[derive(Debug, Clone)]
struct MonitorConfig {
    name: String,
    position: (i32, i32),
}

impl MonitorConfig {
    fn to_kdl(&self) -> kdl::KdlNode {
        let mut node = kdl::KdlNode::new("monitor");
        node.push(self.name.clone());

        let mut children = kdl::KdlDocument::new();
        let mut position_node = kdl::KdlNode::new("position");
        position_node.push(self.position.0 as i128);
        position_node.push(self.position.1 as i128);
        children.nodes_mut().push(position_node);

        node.set_children(children);
        node
    }

    fn from_kdl(node: &kdl::KdlNode) -> Option<Self> {
        let name = node.entries().first()?.value().as_string()?.to_string();

        let children = node.children()?;
        for child in children.nodes() {
            if child.name().value() == "position" {
                if let [x, y, ..] = child.entries() {
                    let position = (
                        x.value().as_integer()? as i32,
                        y.value().as_integer()? as i32,
                    );
                    return Some(MonitorConfig { name, position });
                }
            }
        }
        None
    }
}

struct MonitorState {
    monitors: Vec<MonitorConfig>,
}

impl MonitorState {
    fn config_path() -> PathBuf {
        let home = std::env::var("HOME").expect("HOME environment variable not set");
        PathBuf::from(home)
            .join(".config")
            .join("cosmic-monitor-hack")
            .join("state.kdl")
    }

    fn save(&self) -> std::io::Result<()> {
        let config_path = Self::config_path();
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut doc = kdl::KdlDocument::new();
        for monitor in &self.monitors {
            doc.nodes_mut().push(monitor.to_kdl());
        }

        fs::write(config_path, doc.to_string())?;
        Ok(())
    }

    fn load() -> std::io::Result<Self> {
        let config_path = Self::config_path();
        let content = fs::read_to_string(config_path)?;
        let document: kdl::KdlDocument = content
            .parse()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let mut monitors = Vec::new();
        for node in document.nodes() {
            if node.name().value() == "monitor" {
                if let Some(monitor) = MonitorConfig::from_kdl(node) {
                    monitors.push(monitor);
                }
            }
        }

        Ok(MonitorState { monitors })
    }

    fn from_current() -> Option<Self> {
        let output = Command::new("cosmic-randr")
            .args(["list", "--kdl"])
            .output()
            .ok()?;

        let outputs_str = std::str::from_utf8(&output.stdout).ok()?;
        let document: kdl::KdlDocument = outputs_str.parse().ok()?;

        let mut monitors = Vec::new();

        for node in document.nodes() {
            if node.name().value() != "output" {
                continue;
            }

            let output_name = node.entries().first()?.value().as_string()?;

            let is_enabled = node
                .entries()
                .iter()
                .skip(1)
                .find(|e| e.name().map(|n| n.value()) == Some("enabled"))
                .and_then(|e| e.value().as_bool())
                .unwrap_or(false);

            if !is_enabled {
                continue;
            }

            // Get position from children
            if let Some(children) = node.children() {
                for child in children.nodes() {
                    if child.name().value() == "position" {
                        if let [x, y, ..] = child.entries() {
                            let position = (
                                x.value().as_integer().unwrap_or(0) as i32,
                                y.value().as_integer().unwrap_or(0) as i32,
                            );
                            monitors.push(MonitorConfig {
                                name: output_name.to_string(),
                                position,
                            });
                        }
                    }
                }
            }
        }

        Some(MonitorState { monitors })
    }
}

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
    _loop_handle: LoopHandle<'static, Self>,
}

impl State {
    fn handle_idle(&mut self) {
        println!("idling!");
    }

    fn handle_resume(&mut self) {
        println!("resuming!");

        // Spawn monitor positioning in a separate thread to not block the event loop
        std::thread::spawn(|| {
            if let Err(e) = apply_monitor_config() {
                eprintln!("Failed to apply monitor configuration: {}", e);
            }
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

fn get_backoff_delay(attempt: usize) -> Duration {
    // Backoff sequence: [2, 4, 8, 16, 30, 30, ...]
    let seconds = match attempt {
        0..=3 => 2u64.pow(attempt as u32 + 1), // 2, 4, 8, 16
        _ => 30,                                 // 30 for all remaining attempts
    };
    Duration::from_secs(seconds)
}

fn check_monitors(
    document: &kdl::KdlDocument,
    expected_monitors: &[MonitorConfig],
) -> HashMap<String, bool> {
    let mut found = HashMap::new();

    for expected in expected_monitors {
        found.insert(expected.name.clone(), false);
    }

    for node in document.nodes() {
        if node.name().value() != "output" {
            continue;
        }

        if let Some(output_name) = node.entries().first().and_then(|e| e.value().as_string()) {
            let is_enabled = node
                .entries()
                .iter()
                .skip(1)
                .find(|e| e.name().map(|n| n.value()) == Some("enabled"))
                .and_then(|e| e.value().as_bool())
                .unwrap_or(false);

            if is_enabled && found.contains_key(output_name) {
                found.insert(output_name.to_string(), true);
            }
        }
    }

    found
}

fn apply_monitor_config() -> Result<(), String> {
    let state = MonitorState::load().map_err(|e| format!("Failed to load state: {}", e))?;

    if state.monitors.is_empty() {
        return Err("No monitors configured".to_string());
    }

    println!(
        "Looking for {} monitor(s): {}",
        state.monitors.len(),
        state
            .monitors
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    for attempt in 0..MAX_ATTEMPTS {
        if attempt > 0 {
            let delay = get_backoff_delay(attempt - 1);
            println!(
                "Attempt {}/{}: waiting {:?} before retry...",
                attempt + 1,
                MAX_ATTEMPTS,
                delay
            );
            std::thread::sleep(delay);
        } else {
            println!("Attempt {}/{}: checking monitors...", attempt + 1, MAX_ATTEMPTS);
        }

        // Get list of connected outputs
        let output = Command::new("cosmic-randr")
            .args(["list", "--kdl"])
            .output()
            .map_err(|e| format!("Failed to execute cosmic-randr: {}", e))?;

        let outputs_str = std::str::from_utf8(&output.stdout)
            .map_err(|e| format!("Failed to parse cosmic-randr output: {}", e))?;

        let document: kdl::KdlDocument = outputs_str
            .parse()
            .map_err(|e| format!("Failed to parse KDL document: {}", e))?;

        let found_monitors = check_monitors(&document, &state.monitors);
        let all_found = found_monitors.values().all(|&v| v);

        // Log status of each monitor
        for (name, found) in &found_monitors {
            let status = if *found { "✓" } else { "✗" };
            println!("  {} {}", status, name);
        }

        if all_found {
            println!("All monitors detected! Applying configuration...");

            // Apply positions for all monitors
            for monitor in &state.monitors {
                if let Err(e) = Command::new("cosmic-randr")
                    .args([
                        "position",
                        &monitor.name,
                        &monitor.position.0.to_string(),
                        &monitor.position.1.to_string(),
                    ])
                    .status()
                {
                    eprintln!("Failed to position {}: {}", monitor.name, e);
                } else {
                    println!(
                        "Positioned {} at ({}, {})",
                        monitor.name, monitor.position.0, monitor.position.1
                    );
                }
            }

            println!("Monitor configuration applied successfully!");
            return Ok(());
        }
    }

    Err(format!(
        "Timeout: Not all monitors detected after {} attempts",
        MAX_ATTEMPTS
    ))
}

fn save_current_config() -> Result<(), String> {
    let state =
        MonitorState::from_current().ok_or("Failed to get current monitor configuration")?;

    if state.monitors.is_empty() {
        return Err("No enabled monitors found".to_string());
    }

    println!("Saving configuration for {} monitor(s):", state.monitors.len());
    for monitor in &state.monitors {
        println!(
            "  {} at ({}, {})",
            monitor.name, monitor.position.0, monitor.position.1
        );
    }

    state
        .save()
        .map_err(|e| format!("Failed to save configuration: {}", e))?;

    println!("Configuration saved to {:?}", MonitorState::config_path());
    Ok(())
}

fn run_monitor_mode() {
    // Check if state file exists, if not, save current configuration
    if !MonitorState::config_path().exists() {
        println!("No saved configuration found, saving current state...");
        if let Err(e) = save_current_config() {
            eprintln!("Warning: {}", e);
            eprintln!("Continuing in monitor mode without saved state...");
            return;
        }
    }

    // Run monitor positioning on startup
    println!("Running initial monitor positioning check...");
    if let Err(e) = apply_monitor_config() {
        eprintln!("Warning: {}", e);
    }

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
        _loop_handle: event_loop.handle(),
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

fn main() {
    env_logger::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Save => {
            if let Err(e) = save_current_config() {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Apply => {
            if let Err(e) = apply_monitor_config() {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Monitor => {
            run_monitor_mode();
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
