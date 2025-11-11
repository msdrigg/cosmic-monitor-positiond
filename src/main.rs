#![allow(clippy::single_match)]

use calloop::{EventLoop, LoopHandle, LoopSignal};
use calloop_wayland_source::WaylandSource;
use clap::{Parser, Subcommand};
use cosmic_randr::context::HeadConfiguration;
use cosmic_randr::{Context, Message};
use std::{fs, path::PathBuf, time::Duration};
use tachyonix::Receiver;
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
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
            if child.name().value() == "position"
                && let [x, y, ..] = child.entries()
            {
                let position = (
                    x.value().as_integer()? as i32,
                    y.value().as_integer()? as i32,
                );
                return Some(MonitorConfig { name, position });
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
            if node.name().value() == "monitor"
                && let Some(monitor) = MonitorConfig::from_kdl(node)
            {
                monitors.push(monitor);
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
        _ => 30,                               // 30 for all remaining attempts
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

async fn apply_monitor_config_once() -> Result<(), String> {
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

        return Ok(());
    }

    Err("Not all monitors detected".to_string())
}

async fn apply_monitor_config_async(
    mut reposition_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
) -> Result<(), String> {
    let mut attempt_counter = 0;
    let timer = tokio::time::sleep(Duration::from_secs(0));
    tokio::pin!(timer);

    loop {
        tokio::select! {
            _ = reposition_rx.recv() => {
                while let Ok(_) = reposition_rx.try_recv() {
                    // Drain any additional reposition requests
                }
                attempt_counter = 0;
                timer.as_mut().reset(tokio::time::Instant::now());
            }
            _ = &mut timer => {
                attempt_counter += 1;
                if attempt_counter > MAX_ATTEMPTS {
                    timer.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(86400 * 365));
                }
                println!("Reposition attempt #{}/{}", attempt_counter, MAX_ATTEMPTS);

                match apply_monitor_config_once().await {
                    Ok(_) => {
                        println!("Monitor configuration applied successfully after {} attempt(s)", attempt_counter);
                        attempt_counter = 0; // Reset counter after success
                    }
                    Err(e) => {
                        eprintln!("Attempt {} failed: {}", attempt_counter, e);
                        if attempt_counter >= MAX_ATTEMPTS {
                            return Err(format!("Maximum attempts ({}) reached without success", MAX_ATTEMPTS));
                        }
                    }
                }

                // Schedule next attempt
                let delay = get_backoff_delay(attempt_counter - 1);
                // timer = tokio::time::sleep(delay);
                timer.as_mut().reset(tokio::time::Instant::now() + delay);
        }
        _ = tokio::signal::ctrl_c() => {
                println!("Ctrl-C received, exiting monitor repositioning task");
                return Ok(());
            }
        }
    }
}

async fn save_current_config() -> Result<(), String> {
    let state = MonitorState::from_current()
        .await
        .ok_or("Failed to get current monitor configuration")?;

    if state.monitors.is_empty() {
        return Err("No enabled monitors found".to_string());
    }

    println!(
        "Saving configuration for {} monitor(s):",
        state.monitors.len()
    );
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
    reposition_handler: tokio::sync::mpsc::UnboundedSender<()>,
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

    #[cfg(feature = "autodetect")]
    fn handle_hotplug(&mut self) {
        println!("Display hotplug detected!");
        self.trigger_reposition();
    }

    fn trigger_reposition(&mut self) {
        if let Err(err) = self.reposition_handler.send(()) {
            // Monitor has exited, we should exit
            eprintln!("Reposition handler error: {}", err);
            self.loop_signal.stop();
        }
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

fn setup_udev_monitor(
    loop_handle: &LoopHandle<'static, IdleMonitorState>,
) -> Result<(), Box<dyn std::error::Error>> {
    let builder = udev::MonitorBuilder::new()?;
    let builder = builder.match_subsystem("drm")?;
    let socket = builder.listen()?;

    let generic =
        calloop::generic::Generic::new(socket, calloop::Interest::READ, calloop::Mode::Level);

    loop_handle.insert_source(generic, |_readiness, socket, state| {
        // Drain all events from the socket
        if socket.iter().next().is_some() {
            // Hotplug event detected
            state.handle_hotplug();
        }

        Ok(calloop::PostAction::Continue)
    })?;

    Ok(())
}

fn run_monitor_mode(rt: tokio::runtime::Runtime) -> Result<(), Box<dyn std::error::Error>> {
    // Check if state file exists, if not, save current configuration
    if !MonitorState::config_path().exists() {
        println!("No saved configuration found, saving current state...");
        if let Err(e) = rt.block_on(save_current_config()) {
            eprintln!("Warning: {}", e);
            eprintln!("Continuing in monitor mode without saved state...");
        }
    }

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
    let (reposition_tx, reposition_rx) = tokio::sync::mpsc::unbounded_channel();

    let mut state = IdleMonitorState {
        idle_notifier,
        seat,
        idle_notification: None,
        reposition_handler: reposition_tx,
        loop_signal: event_loop.get_signal().clone(),
    };

    // Run monitor positioning on startup
    println!("Running initial monitor positioning check...");
    if let Err(e) = rt.block_on(apply_monitor_config_once()) {
        eprintln!("Warning: {}", e);
    }

    // Create initial idle notification
    state.recreate_notification(&qh);

    // Setup Wayland event source
    WaylandSource::new(connection, event_queue).insert(event_loop.handle())?;

    println!("Monitoring for idle/resume events...");

    // Setup udev monitor for DRM hotplug events (if feature is enabled)

    #[cfg(feature = "autodetect")]
    {
        if let Err(e) = setup_udev_monitor(&event_loop.handle()) {
            eprintln!("Warning: Failed to setup udev monitoring: {}", e);

            eprintln!("Hotplug detection will not be available");
        } else {
            println!("Udev hotplug monitoring enabled");
        }
    }

    rt.spawn(async move {
        // Select between ctrl_c and reposition requests...
        match apply_monitor_config_async(reposition_rx).await {
            Ok(_) => {
                println!("Monitor repositioning task exited");
            }
            Err(e) => {
                eprintln!("Error applying monitor configuration: {}", e);
            }
        }
    });

    // Run tokio event loop
    event_loop.run(None, &mut state, |_| {})?;

    Ok(())
}

// #[tokio::main(flavor = "current_thread")]
fn main() {
    env_logger::init();

    let cli = Cli::parse();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .max_blocking_threads(1)
        .worker_threads(1)
        .build()
        .expect("Failed to create Tokio runtime");

    match cli.command {
        Commands::Save => {
            if let Err(e) = rt.block_on(save_current_config()) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Apply => {
            if let Err(e) = rt.block_on(apply_monitor_config_once()) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            } else {
                println!("Monitor configuration applied successfully!");
            }
        }
        Commands::Monitor => {
            if let Err(e) = run_monitor_mode(rt) {
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
