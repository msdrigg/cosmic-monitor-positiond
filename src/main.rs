#![allow(clippy::single_match)]

use calloop::{timer::{Timer, TimeoutAction}, EventLoop, LoopHandle, LoopSignal};
use calloop_wayland_source::WaylandSource;
use clap::{Parser, Subcommand};
use cosmic_randr::context::HeadConfiguration;
use cosmic_randr::{Context, Message};
use std::{fs, path::PathBuf, time::Duration};
use tachyonix::Receiver;
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{
    delegate_noop,
    globals::{registry_queue_init, GlobalListContents},
    Connection, Dispatch, EventQueue, QueueHandle,
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

    async fn from_current() -> Option<Self> {
        let (message_tx, mut message_rx) = tachyonix::channel(5);
        let (mut context, mut event_queue) = cosmic_randr::connect(message_tx).ok()?;

        // Wait for manager done
        dispatch_until_manager_done(&mut context, &mut event_queue, &mut message_rx)
            .await
            .ok()?;

        let mut monitors = Vec::new();

        for head in context.output_heads.values() {
            if !head.enabled {
                continue;
            }

            monitors.push(MonitorConfig {
                name: head.name.clone(),
                position: (head.position_x, head.position_y),
            });
        }

        Some(MonitorState { monitors })
    }
}

async fn dispatch_until_manager_done(
    context: &mut Context,
    event_queue: &mut EventQueue<Context>,
    message_rx: &mut Receiver<Message>,
) -> Result<(), cosmic_randr::Error> {
    loop {
        let watcher = async {
            while let Ok(msg) = message_rx.recv().await {
                if matches!(msg, Message::ManagerDone) {
                    return true;
                }
            }
            false
        };

        tokio::select! {
            is_done = watcher => {
                if is_done {
                    break;
                }
            },
            result = context.dispatch(event_queue) => {
                result?;
            }
        };
    }

    Ok(())
}

async fn receive_config_messages(
    context: &mut Context,
    event_queue: &mut EventQueue<Context>,
    message_rx: &mut Receiver<Message>,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        while let Ok(message) = message_rx.try_recv() {
            match message {
                Message::ConfigurationCancelled => return Err("configuration cancelled".into()),
                Message::ConfigurationFailed => return Err("configuration failed".into()),
                Message::ConfigurationSucceeded => return Ok(()),
                _ => {}
            }
        }

        context.dispatch(event_queue).await?;
    }
}

async fn set_position(
    context: &mut Context,
    event_queue: &mut EventQueue<Context>,
    message_rx: &mut Receiver<Message>,
    name: &str,
    x: i32,
    y: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = context.create_output_config();
    config.enable_head(
        name,
        Some(HeadConfiguration {
            pos: Some((x, y)),
            ..Default::default()
        }),
    )?;

    config.apply();

    receive_config_messages(context, event_queue, message_rx).await
}

fn get_backoff_delay(attempt: usize) -> Duration {
    // Backoff sequence: [2, 4, 8, 16, 30, 30, ...]
    let seconds = match attempt {
        0..=3 => 2u64.pow(attempt as u32 + 1), // 2, 4, 8, 16
        _ => 30,                                 // 30 for all remaining attempts
    };
    Duration::from_secs(seconds)
}

async fn check_monitors_present(
    context: &Context,
    expected_monitors: &[MonitorConfig],
) -> Vec<(String, bool)> {
    let mut found = Vec::new();

    for expected in expected_monitors {
        let is_present = context
            .output_heads
            .values()
            .any(|head| head.name == expected.name && head.enabled);
        found.push((expected.name.clone(), is_present));
    }

    found
}

async fn apply_monitor_config_async() -> Result<(), String> {
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
            tokio::time::sleep(delay).await;
        } else {
            println!("Attempt {}/{}: checking monitors...", attempt + 1, MAX_ATTEMPTS);
        }

        // Create cosmic-randr connection
        let (message_tx, mut message_rx) = tachyonix::channel(5);
        let (mut context, mut event_queue) = cosmic_randr::connect(message_tx)
            .map_err(|e| format!("Failed to connect to cosmic-randr: {}", e))?;

        // Wait for manager done to get current state
        dispatch_until_manager_done(&mut context, &mut event_queue, &mut message_rx)
            .await
            .map_err(|e| format!("Failed to get display list: {}", e))?;

        let found_monitors = check_monitors_present(&context, &state.monitors).await;
        let all_found = found_monitors.iter().all(|(_, found)| *found);

        // Log status of each monitor
        for (name, found) in &found_monitors {
            let status = if *found { "✓" } else { "✗" };
            println!("  {} {}", status, name);
        }

        if all_found {
            println!("All monitors detected! Applying configuration...");

            // Apply positions for all monitors
            for monitor in &state.monitors {
                // Create new connection for each position command
                let (message_tx, mut message_rx) = tachyonix::channel(5);
                let (mut context, mut event_queue) = cosmic_randr::connect(message_tx)
                    .map_err(|e| format!("Failed to connect: {}", e))?;

                dispatch_until_manager_done(&mut context, &mut event_queue, &mut message_rx)
                    .await
                    .map_err(|e| format!("Failed to initialize: {}", e))?;

                if let Err(e) = set_position(
                    &mut context,
                    &mut event_queue,
                    &mut message_rx,
                    &monitor.name,
                    monitor.position.0,
                    monitor.position.1,
                )
                .await
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

async fn save_current_config() -> Result<(), String> {
    let state = MonitorState::from_current()
        .await
        .ok_or("Failed to get current monitor configuration")?;

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

// Idle/resume monitoring state
struct IdleMonitorState {
    idle_notifier: ext_idle_notifier_v1::ExtIdleNotifierV1,
    seat: wl_seat::WlSeat,
    idle_notification: Option<IdleNotification>,
    loop_handle: LoopHandle<'static, Self>,
    loop_signal: LoopSignal,
}

struct IdleNotification {
    notification: ext_idle_notification_v1::ExtIdleNotificationV1,
}

impl IdleNotification {
    fn new(
        idle_notifier: &ext_idle_notifier_v1::ExtIdleNotifierV1,
        seat: &wl_seat::WlSeat,
        timeout_ms: u32,
        qh: &QueueHandle<IdleMonitorState>,
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

impl IdleMonitorState {
    fn handle_idle(&mut self) {
        println!("idling!");
    }

    fn handle_resume(&mut self) {
        println!("resuming!");
        self.trigger_reposition();
    }

    fn trigger_reposition(&mut self) {
        let loop_handle = self.loop_handle.clone();

        // Start the apply process with retries
        let timer = Timer::from_duration(Duration::from_secs(0));
        let _ = loop_handle.insert_source(timer, move |_event, _metadata, _shared_data| {
            // Spawn async task to apply monitors
            tokio::spawn(async move {
                if let Err(e) = apply_monitor_config_async().await {
                    eprintln!("Failed to apply monitor configuration: {}", e);
                }
            });

            TimeoutAction::Drop
        });
    }

    fn recreate_notification(&mut self, qh: &QueueHandle<Self>) {
        self.idle_notification = Some(IdleNotification::new(
            &self.idle_notifier,
            &self.seat,
            IDLE_TIMEOUT_MS,
            qh,
        ));
    }
}

fn run_monitor_mode() -> Result<(), Box<dyn std::error::Error>> {
    // Check if state file exists, if not, save current configuration
    if !MonitorState::config_path().exists() {
        println!("No saved configuration found, saving current state...");
        let runtime = tokio::runtime::Runtime::new()?;
        runtime.block_on(async {
            if let Err(e) = save_current_config().await {
                eprintln!("Warning: {}", e);
                eprintln!("Continuing in monitor mode without saved state...");
            }
        });
    }

    // Run monitor positioning on startup
    println!("Running initial monitor positioning check...");
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        if let Err(e) = apply_monitor_config_async().await {
            eprintln!("Warning: {}", e);
        }
    });

    // Setup Wayland connection for idle monitoring
    let connection = Connection::connect_to_env()?;
    let (globals, event_queue) = registry_queue_init::<IdleMonitorState>(&connection)?;
    let qh = event_queue.handle();

    // Bind to required Wayland protocols
    let idle_notifier = globals
        .bind::<ext_idle_notifier_v1::ExtIdleNotifierV1, _, _>(&qh, 1..=1, ())
        .expect("ext-idle-notifier-v1 not available");

    let seat = globals
        .bind::<wl_seat::WlSeat, _, _>(&qh, 1..=1, ())
        .expect("wl_seat not available");

    // Create event loop
    let mut event_loop: EventLoop<IdleMonitorState> = EventLoop::try_new()?;
    let loop_signal = event_loop.get_signal();

    let mut state = IdleMonitorState {
        idle_notifier,
        seat,
        idle_notification: None,
        loop_handle: event_loop.handle(),
        loop_signal: loop_signal.clone(),
    };

    // Create initial idle notification
    state.recreate_notification(&qh);

    // Setup Wayland event source
    WaylandSource::new(connection, event_queue).insert(event_loop.handle())?;

    println!("Monitoring for idle/resume events...");

    // Run event loop
    event_loop.run(None, &mut state, |_| {})?;

    Ok(())
}

#[tokio::main]
async fn main() {
    env_logger::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Save => {
            if let Err(e) = save_current_config().await {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Apply => {
            if let Err(e) = apply_monitor_config_async().await {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Monitor => {
            if let Err(e) = run_monitor_mode() {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for IdleMonitorState {
    fn event(
        _state: &mut Self,
        _: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ext_idle_notification_v1::ExtIdleNotificationV1, ()> for IdleMonitorState {
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

delegate_noop!(IdleMonitorState: ignore wl_seat::WlSeat);
delegate_noop!(IdleMonitorState: ext_idle_notifier_v1::ExtIdleNotifierV1);
