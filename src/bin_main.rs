//! Overbridge Host — binary entry (`ob-host run`).

use anyhow::{Context, Result};
use clap::Parser;
use crossbeam_channel::unbounded;
use std::future::IntoFuture;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

use truce_rack_core::scanner::PluginScanner;
use truce_rack_vst3::{set_editor_open_notifier, set_param_change_notifier, set_param_refresh_notifier};

use crate::api;
use crate::config::AppConfig;
use crate::engine;
use crate::host::PluginHost;
use crate::midi;
use crate::net_util;
use crate::scenes_store;
use crate::state::AppState;

#[derive(Parser, Debug)]
#[command(name = "ob-host", about = "Elektron Overbridge VST host with control API")]
pub struct Cli {
    /// Plugin name substring (e.g. Digitakt, Syntakt)
    #[arg(long, env = "OB_PLUGIN")]
    pub plugin: Option<String>,

    /// Path to config JSON
    #[arg(long, default_value = "config/default.json")]
    pub config: PathBuf,

    /// Path to MIDI mapping config
    #[arg(long, default_value = "config/mappings.example.json")]
    pub mappings: PathBuf,

    /// API listen port
    #[arg(long, env = "OB_PORT")]
    pub port: Option<u16>,

    /// Plugin scan directory
    #[arg(long, env = "OB_PLUGIN_DIR")]
    pub plugin_dir: Option<PathBuf>,

    /// Skip launching Overbridge Engine
    #[arg(long)]
    pub no_engine: bool,

    /// Use the in-process fake plugin (for headless parameter testing)
    #[arg(long, env = "OB_FAKE_PLUGIN")]
    pub fake_plugin: bool,

    /// Open the plugin editor in a native window (needed for hardware parameter sync)
    #[arg(long, env = "OB_GUI")]
    pub gui: bool,

    /// List available plugins and exit
    #[arg(long)]
    pub list_plugins: bool,

    /// Enable debug UI (MIDI message log in the scenes surface)
    #[arg(long, env = "OB_DEBUG")]
    pub debug: bool,
}

#[tokio::main(flavor = "current_thread")]
pub async fn run() -> Result<()> {
    #[cfg(target_os = "macos")]
    crate::host::editor_macos::init_appkit();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    run_with_cli(cli).await
}

pub async fn run_with_cli(cli: Cli) -> Result<()> {
    let mut cfg = AppConfig::load(&cli.config).context("load config")?;

    if let Some(port) = cli.port {
        cfg.api_port = port;
    }
    if let Some(dir) = cli.plugin_dir {
        cfg.plugin_dir = dir.to_string_lossy().into_owned();
    }

    let plugin_dir = resolve_path(PathBuf::from(&cfg.plugin_dir));

    if !cli.fake_plugin && !plugin_dir.exists() {
        anyhow::bail!(
            "plugin directory not found: {} — run scripts/setup.sh first",
            plugin_dir.display()
        );
    }

    let scanner = truce_rack::vst3::Vst3Scanner::new();
    let plugins = if cli.fake_plugin {
        Vec::new()
    } else {
        scanner
            .scan_path(&plugin_dir)
            .context("scan plugins directory")?
    };

    if cli.list_plugins {
        println!("Available Overbridge plugins in {}:", plugin_dir.display());
        for p in &plugins {
            println!("  - {} ({})", p.name, p.unique_id);
        }
        return Ok(());
    }

    let (editor_open_tx, editor_open_rx) = unbounded();
    let (param_change_tx, param_change_rx) = unbounded();
    let (param_refresh_tx, param_refresh_rx) = unbounded();
    set_editor_open_notifier(editor_open_tx);
    set_param_change_notifier(param_change_tx);
    set_param_refresh_notifier(param_refresh_tx);

    let (plugin_info, host) = if cli.fake_plugin {
        tracing::info!("Using in-process fake plugin (OB Test Host)");
        let host = PluginHost::start_fake(editor_open_rx, param_change_rx, param_refresh_rx)
            .context("start fake plugin host")?;
        let info = host.plugin_info().clone();
        (info, host)
    } else {
        let plugin_name = cli
            .plugin
            .or(cfg.default_plugin.clone())
            .context("no plugin specified — use --plugin or set default_plugin in config")?;

        let plugin_info = plugins
            .iter()
            .find(|p| {
                p.name
                    .to_ascii_lowercase()
                    .contains(&plugin_name.to_ascii_lowercase())
            })
            .with_context(|| {
                format!(
                    "plugin matching '{plugin_name}' not found in {}",
                    plugin_dir.display()
                )
            })?;

        tracing::info!("Loading plugin: {}", plugin_info.name);

        let engine_path = std::env::var("OB_OVERBRIDGE_ENGINE")
            .unwrap_or_else(|_| cfg.overbridge_engine.clone());

        if !cli.no_engine {
            engine::ensure_overbridge_engine(&engine_path)?;
        }

        let instance = scanner
            .load(plugin_info)
            .context("load VST3 plugin — is Overbridge Engine running and device connected?")?;

        let host = PluginHost::start_vst3(
            instance,
            editor_open_rx,
            param_change_rx,
            param_refresh_rx,
            cli.gui,
        )
        .context("start plugin host")?;

        (plugin_info.clone(), host)
    };

    let mappings = midi::MapperConfig::load(&cli.mappings).unwrap_or_default();
    let midi = if cfg.midi.enabled && !cli.fake_plugin {
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
    let midi_monitor = if cli.fake_plugin {
        None
    } else {
        match midi::MidiMonitor::start(midi_tx.clone()) {
            Ok(m) => Some(m),
            Err(e) => {
                tracing::warn!("MIDI monitor unavailable: {e:#}");
                None
            }
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
