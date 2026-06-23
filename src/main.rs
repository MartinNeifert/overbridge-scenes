//! Overbridge Host — local VST host with programmatic control API.
//!
//! Loads Elektron Overbridge VST3 plugins from the bundled `plugins/`
//! directory, drives real-time audio via cpal, and exposes HTTP +
//! WebSocket endpoints for parameter control, MIDI routing, and
//! physical controller mapping.

mod api;
mod config;
mod devices;
mod engine;
mod host;
mod match_devices;
mod midi;
mod net_util;
mod scenes_store;
mod state;

use anyhow::{Context, Result};
use clap::Parser;
use crossbeam_channel::unbounded;
use std::future::IntoFuture;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

use truce_rack_core::scanner::PluginScanner;
use truce_rack_vst3::{set_editor_open_notifier, set_param_change_notifier, set_param_refresh_notifier};

use crate::config::AppConfig;
use crate::host::{resolve_audio_device, list_output_devices, PluginHost};
use crate::state::AppState;

#[derive(Parser, Debug)]
#[command(name = "ob-host", about = "Elektron Overbridge VST host with control API")]
struct Cli {
    /// Plugin name substring (e.g. Digitakt, Syntakt)
    #[arg(long, env = "OB_PLUGIN")]
    plugin: Option<String>,

    /// Path to config JSON
    #[arg(long, default_value = "config/default.json")]
    config: PathBuf,

    /// Path to MIDI mapping config
    #[arg(long, default_value = "config/mappings.example.json")]
    mappings: PathBuf,

    /// API listen port
    #[arg(long, env = "OB_PORT")]
    port: Option<u16>,

    /// Plugin scan directory
    #[arg(long, env = "OB_PLUGIN_DIR")]
    plugin_dir: Option<PathBuf>,

    /// Skip launching Overbridge Engine
    #[arg(long)]
    no_engine: bool,

    /// Open the plugin editor in a native window (needed for hardware parameter sync)
    #[arg(long, env = "OB_GUI")]
    gui: bool,

    /// Control-only: drive parameter / MIDI control without opening the
    /// Overbridge audio device, so the hardware's own audio output is left
    /// untouched (overrides config `control_only`)
    #[arg(long, env = "OB_CONTROL_ONLY")]
    control_only: bool,

    /// Open the Overbridge audio device for monitoring (overrides
    /// `control_only`; restores the old duplex-audio behavior)
    #[arg(long)]
    audio: bool,

    /// Passthrough: open the Overbridge device and loop its captured input
    /// straight back to its output, so the hardware keeps playing its own audio
    /// while the host stays connected (overrides `control_only`)
    #[arg(long)]
    passthru: bool,

    /// Native duplex audio: host the Elektron device as a single duplex AUHAL
    /// (one device, one clock — the DAW-equivalent path the Overbridge Engine
    /// can measure without faulting) and monitor its audio back to its output.
    /// Optionally takes the device name substring; defaults to config / plugin.
    #[arg(long, value_name = "DEVICE")]
    duplex: Option<Option<String>>,

    /// List available plugins and exit
    #[arg(long)]
    list_plugins: bool,

    /// List cpal output devices and exit
    #[arg(long)]
    list_devices: bool,

    /// Enable debug UI (MIDI message log in the scenes surface)
    #[arg(long, env = "OB_DEBUG")]
    debug: bool,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    #[cfg(target_os = "macos")]
    host::editor_macos::init_appkit();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let mut cfg = AppConfig::load(&cli.config).context("load config")?;

    if let Some(port) = cli.port {
        cfg.api_port = port;
    }
    if let Some(dir) = cli.plugin_dir {
        cfg.plugin_dir = dir.to_string_lossy().into_owned();
    }

    let plugin_dir = resolve_path(PathBuf::from(&cfg.plugin_dir));
    if !plugin_dir.exists() {
        anyhow::bail!(
            "plugin directory not found: {} — run scripts/setup.sh first",
            plugin_dir.display()
        );
    }

    let scanner = truce_rack::vst3::Vst3Scanner::new();
    let plugins = scanner
        .scan_path(&plugin_dir)
        .context("scan plugins directory")?;

    if cli.list_devices {
        println!("cpal output devices:");
        for name in list_output_devices().context("list output devices")? {
            println!("  - {name}");
        }
        return Ok(());
    }

    if cli.list_plugins {
        println!("Available Overbridge plugins in {}:", plugin_dir.display());
        for p in &plugins {
            println!("  - {} ({})", p.name, p.unique_id);
        }
        return Ok(());
    }

    let plugin_name = cli
        .plugin
        .or(cfg.default_plugin.clone())
        .context("no plugin specified — use --plugin or set default_plugin in config")?;

    let plugin_info = plugins
        .iter()
        .find(|p| p.name.to_ascii_lowercase().contains(&plugin_name.to_ascii_lowercase()))
        .with_context(|| format!("plugin matching '{plugin_name}' not found in {}", plugin_dir.display()))?;

    tracing::info!("Loading plugin: {}", plugin_info.name);

    if !cli.no_engine {
        engine::ensure_overbridge_engine(&cfg.overbridge_engine)?;
    }

    let (editor_open_tx, editor_open_rx) = unbounded();
    let (param_change_tx, param_change_rx) = unbounded();
    let (param_refresh_tx, param_refresh_rx) = unbounded();
    set_editor_open_notifier(editor_open_tx);
    set_param_change_notifier(param_change_tx);
    set_param_refresh_notifier(param_refresh_tx);

    let instance = scanner
        .load(plugin_info)
        .context("load VST3 plugin — is Overbridge Engine running and device connected?")?;

    // Audio modes:
    //   default (control-only) — never engage the audio engine: no device opened,
    //                   no setActive/process(). Parameters go through the edit
    //                   controller only, so the hardware keeps its own audio.
    //   --audio       — open a duplex stream and send the plugin's processed
    //                   output to the device (this overrides the device's audio).
    let monitor = cli.audio;
    let passthru = cli.passthru;

    // Native duplex mode (the working DAW-equivalent path). Enabled by `--duplex`
    // or `duplex.enabled` in config. The device hint comes from the CLI value,
    // then config, then the plugin name. Monitoring (device audio routed back to
    // its own output) defaults on so the analog Main Out stays audible.
    let duplex_cli = cli.duplex.is_some();
    let duplex = if duplex_cli || cfg.duplex.enabled {
        let device = cli
            .duplex
            .flatten()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                Some(cfg.duplex.device.clone()).filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| plugin_info.name.clone());
        Some(host::DuplexSettings {
            device,
            monitor: cfg.duplex.monitor,
            monitor_source: cfg.duplex.monitor_source,
            monitor_gain: cfg.duplex.monitor_gain,
        })
    } else {
        None
    };

    // Duplex supersedes the control-only / clock fallbacks.
    let control_only = duplex.is_none()
        && !cli.audio
        && !cli.passthru
        && (cli.control_only || cfg.control_only);

    // Device resolution. The Elektron hardware presents as a CoreAudio device,
    // but the Overbridge Engine OWNS that device — opening it from here (input or
    // output) contends with the Engine and cuts the hardware audio. A DAW never
    // opens the Elektron device for a control/snapshot workflow; it clocks the
    // plugin's process() from its OWN interface while the Engine streams the
    // hardware. So in the default (engine/clock) mode we deliberately do NOT
    // resolve the Elektron device — the audio engine will pick a neutral output
    // device as a steady clock instead. Only --audio/--passthru, which explicitly
    // route audio to/from the device, open the Elektron device directly.
    let audio_device = if monitor || passthru {
        match resolve_audio_device(&cfg, &plugin_info.name) {
            Ok(dev) => Some(dev),
            Err(e) => return Err(e).context("open Overbridge audio device"),
        }
    } else {
        None
    };

    if let Some(d) = &duplex {
        tracing::info!(
            "Duplex mode: single AUHAL on \"{}\" (one device, one clock; DAW-equivalent), monitor {}",
            d.device,
            if d.monitor { "on" } else { "off" }
        );
    } else if control_only {
        tracing::info!(
            "Control-only mode: audio engine not engaged — control via edit controller only (no process())"
        );
    } else if passthru {
        tracing::info!(
            "Passthrough mode (--passthru): looping device audio back to itself to keep it alive while connected"
        );
    } else if monitor {
        tracing::info!(
            "Monitor mode (--audio): plugin output sent to the Overbridge device (overrides device audio)"
        );
    } else {
        tracing::info!(
            "Engine mode (default): hosting the plugin at its native multibus layout; driving process() from the device clock (silent output) when present, else a timer loop"
        );
    }

    let host = PluginHost::start(
        instance,
        audio_device,
        cfg.block_size,
        editor_open_rx,
        param_change_rx,
        param_refresh_rx,
        cli.gui,
        control_only,
        monitor,
        passthru,
        duplex,
    )
    .context("start audio host")?;

    let mappings = midi::MapperConfig::load(&cli.mappings).unwrap_or_default();
    let midi = if cfg.midi.enabled {
        Some(
            midi::MidiBridge::start(
                &cfg.midi.virtual_port_name,
                host.command_sender(),
                mappings.clone(),
                host.parameter_index(),
            )
            .context("start MIDI bridge")?,
        )
    } else {
        None
    };

    let (midi_tx, _) = tokio::sync::broadcast::channel(512);
    let midi_monitor = match midi::MidiMonitor::start(midi_tx.clone()) {
        Ok(m) => Some(m),
        Err(e) => {
            tracing::warn!("MIDI monitor unavailable: {e:#}");
            None
        }
    };

    let scenes_dir = resolve_path(PathBuf::from("data/scenes"));
    let scenes_store = scenes_store::ScenesStore::new(scenes_dir.clone());
    tracing::info!("Scenes store: {}", scenes_dir.display());

    let state = Arc::new(AppState::new(
        host,
        plugin_info.clone(),
        cfg.clone(),
        plugin_dir.clone(),
        plugins,
        mappings,
        midi,
        midi_tx,
        midi_monitor,
        scenes_store,
        cli.debug,
    ));

    if cli.debug {
        tracing::info!("Debug mode: MIDI message log enabled in web UI");
    }

    let web_dir = resolve_path(PathBuf::from("web"));
    let addr = format!("0.0.0.0:{}", cfg.api_port);
    tracing::info!("Control API listening on http://127.0.0.1:{}", cfg.api_port);
    tracing::info!("Web control surface: http://127.0.0.1:{}/", cfg.api_port);
    tracing::info!("Remote crossfader: http://127.0.0.1:{}/remote.html", cfg.api_port);
    if let Some(ip) = net_util::local_lan_ip() {
        tracing::info!(
            "LAN remote crossfader: http://{}:{}/remote.html",
            ip,
            cfg.api_port
        );
    }
    if let Some(host) = net_util::local_hostname() {
        tracing::info!(
            "LAN remote crossfader: http://{}.local:{}/remote.html",
            host,
            cfg.api_port
        );
    }

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("bind {addr}"))?;

    let app = api::router(state.clone(), web_dir);
    let mut server = std::pin::pin!(axum::serve(listener, app).into_future());

    let mut runloop = tokio::time::interval(std::time::Duration::from_millis(4));
    runloop.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            result = server.as_mut() => {
                result.context("HTTP server")?;
                break;
            }
            _ = runloop.tick() => {
                state.host().runloop_tick();
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Shutting down...");
                state.host().shutdown();
                break;
            }
        }
    }

    Ok(())
}

fn resolve_path(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        return path;
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(path)
}
