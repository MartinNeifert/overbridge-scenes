use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender, unbounded};
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use truce_rack_core::info::{ParameterInfo, PluginInfo};
use truce_rack_core::plugin::PluginCore;
use truce_rack::vst3::Vst3Plugin;

use crate::host::audio::AudioEngine;
use crate::host::audio_device::OverbridgeAudioDevice;

#[cfg(target_os = "macos")]
use crate::host::editor_macos::EditorPump;
#[cfg(target_os = "macos")]
use crate::host::gui_macos::GuiWindow;

/// Snapshot of a single parameter for API / WebSocket responses.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ParameterSnapshot {
    pub index: usize,
    pub id: u32,
    pub name: String,
    pub short_name: String,
    pub unit: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub value: f64,
    pub display: String,
}

/// Commands sent from the API/MIDI threads to the audio host thread.
#[derive(Debug, Clone)]
pub enum HostCommand {
    SetParameter { index: usize, value: f64 },
    SetParameterByName { name: String, value: f64 },
    SendMidiNote { channel: u8, note: u8, velocity: u8, on: bool },
    SendMidiCc { channel: u8, controller: u8, value: u8 },
    SendRawMidi { data: Vec<u8> },
    ApplyMacro { name: String, value: f64 },
    SyncAllParameters,
}

/// Thread-safe parameter name → index lookup, populated at startup.
pub type ParameterIndex = Arc<RwLock<HashMap<String, usize>>>;

pub type SharedPlugin = Arc<Mutex<Vst3Plugin>>;

pub struct PluginHost {
    cmd_tx: Sender<HostCommand>,
    parameters: Arc<RwLock<Vec<ParameterSnapshot>>>,
    param_index: ParameterIndex,
    param_id_to_index: HashMap<u32, usize>,
    shared_plugin: SharedPlugin,
    param_change_rx: Receiver<(u32, f64)>,
    plugin_info: PluginInfo,
    audio_device_name: String,
    audio_channels: u16,
    audio_handle: Option<JoinHandle<()>>,
    shutdown_tx: Option<Sender<()>>,
    #[cfg(target_os = "macos")]
    editor_pump: Option<EditorPump>,
    #[cfg(target_os = "macos")]
    gui_window: Option<GuiWindow>,
    pending_ws: Arc<Mutex<Vec<(usize, f64, String)>>>,
    param_epoch: Arc<std::sync::atomic::AtomicU64>,
}

impl PluginHost {
    pub fn start(
        plugin: Vst3Plugin,
        audio_device: OverbridgeAudioDevice,
        block_size: usize,
        editor_open_rx: Receiver<()>,
        param_change_rx: Receiver<(u32, f64)>,
        param_refresh_rx: Receiver<()>,
        use_gui: bool,
    ) -> Result<Self> {
        let plugin_info = plugin.info().clone();
        let param_count = plugin.parameter_count();
        let mut snapshots = Vec::with_capacity(param_count);
        let mut name_index = HashMap::new();
        let mut param_id_to_index = HashMap::new();

        for i in 0..param_count {
            let info = plugin.parameter_info(i).context("parameter_info")?;
            name_index.insert(info.name.to_ascii_lowercase(), i);
            if !info.short_name.is_empty() {
                name_index.insert(info.short_name.to_ascii_lowercase(), i);
            }
            param_id_to_index.insert(info.id, i);
            snapshots.push(snapshot_from_info(i, &info));
        }

        tracing::info!("Plugin exposes {param_count} parameters");

        let parameters = Arc::new(RwLock::new(snapshots));
        let param_index = Arc::new(RwLock::new(name_index));
        let pending_ws = Arc::new(Mutex::new(Vec::new()));
        let param_epoch = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (cmd_tx, cmd_rx) = unbounded();
        let (shutdown_tx, shutdown_rx) = unbounded();
        let (audio_ready_tx, audio_ready_rx) = unbounded();
        let (param_flush_tx, param_flush_rx) = unbounded();

        let audio_device_name = audio_device.name.clone();
        let audio_channels = audio_device.channels;

        let shared_plugin: SharedPlugin = Arc::new(Mutex::new(plugin));

        let params_for_audio = Arc::clone(&parameters);
        let plugin_for_audio = Arc::clone(&shared_plugin);
        let audio_handle = thread::Builder::new()
            .name("ob-audio".into())
            .spawn(move || {
                if let Err(e) = AudioEngine::run(
                    plugin_for_audio,
                    audio_device,
                    block_size,
                    cmd_rx,
                    shutdown_rx,
                    params_for_audio,
                    audio_ready_tx,
                    param_flush_tx,
                ) {
                    tracing::error!("Audio engine error: {e:#}");
                }
            })
            .context("spawn audio thread")?;

        // Wait for audio activation before opening the editor (Overbridge expects
        // a live processing graph, then editor + main run loop for device IPC).
        audio_ready_rx
            .recv()
            .context("wait for audio activation")?;
        tracing::info!("Audio activated, starting main run-loop pump...");

        #[cfg(target_os = "macos")]
        let skip_editor = std::env::var("OB_NO_EDITOR")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        #[cfg(target_os = "macos")]
        let open_editor = !use_gui && !skip_editor;
        #[cfg(target_os = "macos")]
        let visible_editor = std::env::var("OB_OPEN_EDITOR")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        #[cfg(target_os = "macos")]
        let gui_window = if use_gui {
            tracing::warn!(
                "--gui is experimental: the editor blocks the main thread; use OB_OPEN_EDITOR=1 instead"
            );
            Some(GuiWindow::start(
                Arc::clone(&shared_plugin),
                plugin_info.name.clone(),
            )?)
        } else {
            None
        };

        #[cfg(target_os = "macos")]
        let editor_pump = if use_gui {
            None
        } else {
            Some(EditorPump::start(
                Arc::clone(&shared_plugin),
                Arc::clone(&parameters),
                Arc::clone(&pending_ws),
                Arc::clone(&param_epoch),
                open_editor,
                visible_editor,
                editor_open_rx,
                param_flush_rx,
                param_refresh_rx,
            )?)
        };

        Ok(Self {
            cmd_tx,
            parameters,
            param_index,
            param_id_to_index,
            shared_plugin,
            param_change_rx,
            plugin_info,
            audio_device_name,
            audio_channels,
            audio_handle: Some(audio_handle),
            shutdown_tx: Some(shutdown_tx),
            #[cfg(target_os = "macos")]
            editor_pump,
            #[cfg(target_os = "macos")]
            gui_window,
            pending_ws,
            param_epoch,
        })
    }

    pub fn param_epoch(&self) -> u64 {
        self.param_epoch.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn take_pending_ws_updates(&self) -> Vec<(usize, f64, String)> {
        std::mem::take(&mut *self.pending_ws.lock())
    }

    pub fn command_sender(&self) -> Sender<HostCommand> {
        self.cmd_tx.clone()
    }

    pub fn parameter_index(&self) -> ParameterIndex {
        Arc::clone(&self.param_index)
    }

    pub fn plugin_info(&self) -> &PluginInfo {
        &self.plugin_info
    }

    pub fn audio_device_name(&self) -> &str {
        &self.audio_device_name
    }

    pub fn audio_channels(&self) -> u16 {
        self.audio_channels
    }

    pub fn parameters(&self) -> Vec<ParameterSnapshot> {
        self.parameters.read().clone()
    }

    pub fn get_parameter(&self, index: usize) -> Option<ParameterSnapshot> {
        self.parameters.read().get(index).cloned()
    }

    pub fn find_parameter_by_name(&self, name: &str) -> Option<ParameterSnapshot> {
        let idx = *self.param_index.read().get(&name.to_ascii_lowercase())?;
        self.get_parameter(idx)
    }

    pub fn set_parameter(&self, index: usize, value: f64) -> Result<()> {
        self.cmd_tx
            .send(HostCommand::SetParameter { index, value })
            .context("send set_parameter command")
    }

    pub fn set_parameter_by_name(&self, name: &str, value: f64) -> Result<()> {
        self.cmd_tx
            .send(HostCommand::SetParameterByName {
                name: name.to_string(),
                value,
            })
            .context("send set_parameter_by_name command")
    }

    pub fn send_midi_note(&self, channel: u8, note: u8, velocity: u8, on: bool) -> Result<()> {
        self.cmd_tx
            .send(HostCommand::SendMidiNote {
                channel,
                note,
                velocity,
                on,
            })
            .context("send midi note")
    }

    pub fn send_midi_cc(&self, channel: u8, controller: u8, value: u8) -> Result<()> {
        self.cmd_tx
            .send(HostCommand::SendMidiCc {
                channel,
                controller,
                value,
            })
            .context("send midi cc")
    }

    pub fn send_raw_midi(&self, data: Vec<u8>) -> Result<()> {
        self.cmd_tx
            .send(HostCommand::SendRawMidi { data })
            .context("send raw midi")
    }

    pub fn runloop_tick(&self) {
        while let Ok((id, value)) = self.param_change_rx.try_recv() {
            self.apply_param_value_by_id(id, value);
        }

        #[cfg(target_os = "macos")]
        if let Some(pump) = &self.editor_pump {
            pump.tick();
        }
    }

    fn apply_param_value_by_id(&self, id: u32, value: f64) {
        let Some(&index) = self.param_id_to_index.get(&id) else {
            return;
        };
        let display = format!("{value:.4}");
        {
            let mut params = self.parameters.write();
            if let Some(snap) = params.get_mut(index) {
                if (snap.value - value).abs() <= f64::EPSILON {
                    return;
                }
                snap.value = value;
                snap.display = display.clone();
            }
        }
        self.pending_ws.lock().push((index, value, display));
        self.param_epoch
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.audio_handle.take() {
            let _ = handle.join();
        }
        #[cfg(target_os = "macos")]
        if let Some(pump) = self.editor_pump.take() {
            pump.shutdown();
        }
        #[cfg(target_os = "macos")]
        if let Some(gui) = self.gui_window.take() {
            gui.shutdown();
        }
    }
}

fn snapshot_from_info(index: usize, info: &ParameterInfo) -> ParameterSnapshot {
    let value = info.default;
    ParameterSnapshot {
        index,
        id: info.id,
        name: info.name.clone(),
        short_name: info.short_name.clone(),
        unit: info.unit.clone(),
        min: info.min,
        max: info.max,
        default: info.default,
        value,
        display: format!("{value:.4}"),
    }
}
