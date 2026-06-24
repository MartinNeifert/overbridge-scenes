use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use crossbeam_channel::unbounded;
use parking_lot::{Mutex, RwLock};
use truce_rack_core::info::PluginInfo;
use truce_rack_core::scanner::PluginScanner;
use truce_rack_vst3::{set_editor_open_notifier, set_param_change_notifier, set_param_refresh_notifier};

use crate::config::AppConfig;
use crate::devices;
use crate::host::{self as plugin_host_mod, PluginHost};
use crate::match_devices;
use crate::midi::{MapperConfig, MidiBridge, MidiInputPort, MidiMessageEvent, MidiMonitor};
use crate::scenes_store::ScenesStore;
use tokio::sync::broadcast;

pub struct AppState {
    host: Mutex<PluginHost>,
    plugin_info: RwLock<PluginInfo>,
    config: AppConfig,
    catalog: Vec<PluginInfo>,
    mappings: MapperConfig,
    midi: Mutex<Option<MidiBridge>>,
    midi_broadcast: broadcast::Sender<MidiMessageEvent>,
    midi_monitor: Mutex<Option<MidiMonitor>>,
    scenes_store: ScenesStore,
    debug: bool,
}

impl AppState {
    pub fn new(
        host: PluginHost,
        plugin_info: PluginInfo,
        config: AppConfig,
        _plugin_dir: PathBuf,
        catalog: Vec<PluginInfo>,
        mappings: MapperConfig,
        midi: Option<MidiBridge>,
        midi_broadcast: broadcast::Sender<MidiMessageEvent>,
        midi_monitor: Option<MidiMonitor>,
        scenes_store: ScenesStore,
        debug: bool,
    ) -> Self {
        Self {
            host: Mutex::new(host),
            plugin_info: RwLock::new(plugin_info),
            config,
            catalog,
            mappings,
            midi: Mutex::new(midi),
            midi_broadcast,
            midi_monitor: Mutex::new(midi_monitor),
            scenes_store,
            debug,
        }
    }

    pub fn debug(&self) -> bool {
        self.debug
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    pub fn scenes_store(&self) -> &ScenesStore {
        &self.scenes_store
    }

    pub fn midi_subscribe(&self) -> broadcast::Receiver<MidiMessageEvent> {
        self.midi_broadcast.subscribe()
    }

    pub fn midi_input_ports(&self) -> Vec<MidiInputPort> {
        self.midi_monitor
            .lock()
            .as_ref()
            .map(|m| m.ports().to_vec())
            .unwrap_or_default()
    }

    pub fn host(&self) -> parking_lot::MutexGuard<'_, PluginHost> {
        self.host.lock()
    }

    pub fn plugin_info(&self) -> PluginInfo {
        self.plugin_info.read().clone()
    }

    pub fn selector_options(&self) -> SelectorResponse {
        let loaded = self.plugin_info.read().name.clone();
        let snapshot = devices::discover();

        let connected: Vec<ConnectedOption> = snapshot
            .devices
            .iter()
            .map(|d| {
                let suggested = match_devices::best_plugin_for_device(&d.name, &self.catalog)
                    .map(|p| p.name.clone());
                let linked = match_devices::plugin_name_matches_device(&loaded, &d.name);
                ConnectedOption {
                    device_name: d.name.clone(),
                    manufacturer: d.manufacturer.clone(),
                    suggested_plugin: suggested.clone(),
                    linked,
                    transport: d.transport.clone(),
                    serial: d.serial.clone(),
                }
            })
            .collect();

        let plugins: Vec<PluginOption> = self
            .catalog
            .iter()
            .map(|p| {
                let is_loaded = p.name == loaded;
                let is_connected = snapshot.devices.iter().any(|d| {
                    match_devices::plugin_name_matches_device(&p.name, &d.name)
                });
                PluginOption {
                    name: p.name.clone(),
                    unique_id: p.unique_id.clone(),
                    loaded: is_loaded,
                    connected: is_connected,
                }
            })
            .collect();

        SelectorResponse {
            loaded_plugin: loaded,
            engine_running: snapshot.engine_running,
            parameter_count: self.host.lock().parameters().len(),
            connected,
            plugins,
        }
    }

    pub fn switch_plugin(&self, plugin_name: &str) -> Result<SelectorResponse> {
        let info = self
            .catalog
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(plugin_name))
            .or_else(|| {
                self.catalog.iter().find(|p| {
                    p.name
                        .to_ascii_lowercase()
                        .contains(&plugin_name.to_ascii_lowercase())
                })
            })
            .cloned()
            .with_context(|| format!("plugin '{plugin_name}' not found"))?;

        tracing::info!("Switching plugin to {}", info.name);

        let (editor_open_tx, editor_open_rx) = unbounded();
        let (param_change_tx, param_change_rx) = unbounded();
        let (param_refresh_tx, param_refresh_rx) = unbounded();
        set_editor_open_notifier(editor_open_tx);
        set_param_change_notifier(param_change_tx);
        set_param_refresh_notifier(param_refresh_tx);

        let scanner = truce_rack::vst3::Vst3Scanner::new();
        let instance = scanner
            .load(&info)
            .context("load VST3 plugin — is Overbridge Engine running?")?;

        {
            let mut host = self.host.lock();
            host.shutdown();
        }

        let new_host = PluginHost::start_vst3(
            instance,
            plugin_host_mod::resolve_audio_device(&self.config, &info.name).ok(),
            self.config.block_size,
            editor_open_rx,
            param_change_rx,
            param_refresh_rx,
            false,
            self.config.control_only,
            false,
            false,
            if self.config.duplex.enabled {
                Some(crate::host::DuplexSettings {
                    device: if self.config.duplex.device.is_empty() {
                        info.name.clone()
                    } else {
                        self.config.duplex.device.clone()
                    },
                    monitor: self.config.duplex.monitor,
                    monitor_source: self.config.duplex.monitor_source,
                    monitor_gain: self.config.duplex.monitor_gain,
                })
            } else {
                None
            },
        )
        .context("start audio host")?;

        self.reconnect_midi(&new_host)?;

        *self.host.lock() = new_host;
        *self.plugin_info.write() = info;

        Ok(self.selector_options())
    }

    fn reconnect_midi(&self, host: &PluginHost) -> Result<()> {
        let mut slot = self.midi.lock();
        *slot = None;
        if self.config.midi.enabled {
            *slot = Some(
                MidiBridge::start(
                    &self.config.midi.virtual_port_name,
                    host.command_sender(),
                    self.mappings.clone(),
                    host.parameter_index(),
                )
                .context("restart MIDI bridge")?,
            );
        }
        Ok(())
    }
}

pub type SharedState = Arc<AppState>;

#[derive(Debug, serde::Serialize)]
pub struct SelectorResponse {
    pub loaded_plugin: String,
    pub engine_running: bool,
    pub parameter_count: usize,
    pub connected: Vec<ConnectedOption>,
    pub plugins: Vec<PluginOption>,
}

#[derive(Debug, serde::Serialize)]
pub struct ConnectedOption {
    pub device_name: String,
    pub manufacturer: String,
    pub suggested_plugin: Option<String>,
    pub linked: bool,
    pub transport: Option<String>,
    pub serial: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct PluginOption {
    pub name: String,
    pub unique_id: String,
    pub loaded: bool,
    pub connected: bool,
}

#[derive(Debug, serde::Deserialize)]
pub struct SelectPluginRequest {
    pub plugin: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct SetParameterRequest {
    pub value: f64,
}

#[derive(Debug, serde::Deserialize)]
pub struct SetParameterByNameRequest {
    pub name: String,
    pub value: f64,
}

#[derive(Debug, serde::Deserialize)]
pub struct BatchParamUpdate {
    pub index: usize,
    pub value: f64,
}

#[derive(Debug, serde::Deserialize)]
pub struct BatchSetParametersRequest {
    pub updates: Vec<BatchParamUpdate>,
}

#[derive(Debug, serde::Deserialize)]
pub struct MidiNoteRequest {
    pub channel: u8,
    pub note: u8,
    pub velocity: u8,
    #[serde(default = "default_true")]
    pub on: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, serde::Deserialize)]
pub struct MidiCcRequest {
    pub channel: u8,
    pub controller: u8,
    pub value: u8,
}

#[derive(Debug, serde::Deserialize)]
pub struct RawMidiRequest {
    pub data: Vec<u8>,
}

#[derive(Debug, serde::Serialize)]
pub struct StatusResponse {
    pub plugin: String,
    pub vendor: String,
    pub parameter_count: usize,
    pub engine_running: bool,
    pub api_version: &'static str,
    pub audio_device: String,
    pub audio_channels: u16,
    pub devices: Vec<devices::ConnectedDevice>,
    pub plugin_matches_device: bool,
    pub debug: bool,
    pub api_port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lan_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lan_hostname: Option<String>,
}
