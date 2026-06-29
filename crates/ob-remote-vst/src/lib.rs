use nih_plug::prelude::*;
use parking_lot::Mutex;
use std::sync::Arc;

mod client;

use client::HostClient;

const DEFAULT_PORT: u16 = 7780;

#[derive(Params)]
struct ObRemoteParams {
    #[id = "crossfader"]
    pub crossfader: FloatParam,

    /// ob-host API port (default 7780).
    #[id = "host_port"]
    pub host_port: IntParam,
}

#[derive(Default, Clone)]
struct UiState {
    connected: bool,
    plugin_name: String,
    pattern: String,
    last_applied: usize,
    last_error: String,
}

struct ObRemoteVst {
    params: Arc<ObRemoteParams>,
    #[allow(dead_code)]
    ui: Arc<Mutex<UiState>>,
    last_sent_pos: f32,
    worker_tx: crossbeam_channel::Sender<WorkerMsg>,
}

enum WorkerMsg {
    Ping { port: u16 },
    Apply { port: u16, pos: f32 },
}

impl Default for ObRemoteVst {
    fn default() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded::<WorkerMsg>();
        let ui = Arc::new(Mutex::new(UiState::default()));
        let ui_worker = Arc::clone(&ui);

        std::thread::Builder::new()
            .name("ob-remote-http".into())
            .spawn(move || worker_loop(rx, ui_worker))
            .expect("spawn HTTP worker");

        Self {
            params: Arc::new(ObRemoteParams::default()),
            ui,
            last_sent_pos: f32::NAN,
            worker_tx: tx,
        }
    }
}

fn worker_loop(rx: crossbeam_channel::Receiver<WorkerMsg>, ui: Arc<Mutex<UiState>>) {
    while let Ok(msg) = rx.recv() {
        match msg {
            WorkerMsg::Ping { port } => {
                let client = HostClient::new(port);
                match client.ping() {
                    Ok(plugin) => {
                        let mut state = ui.lock();
                        state.connected = true;
                        state.plugin_name = plugin;
                        state.last_error.clear();
                    }
                    Err(err) => {
                        let mut state = ui.lock();
                        state.connected = false;
                        state.plugin_name.clear();
                        state.last_error = err;
                    }
                }
            }
            WorkerMsg::Apply { port, pos } => {
                let client = HostClient::new(port);
                match client.apply_crossfader(f64::from(pos)) {
                    Ok(resp) => {
                        let mut state = ui.lock();
                        state.connected = true;
                        state.pattern = resp.pattern;
                        state.last_applied = resp.applied;
                        state.last_error.clear();
                    }
                    Err(err) => {
                        let mut state = ui.lock();
                        state.connected = false;
                        state.last_error = err;
                    }
                }
            }
        }
    }
}

impl Plugin for ObRemoteVst {
    const NAME: &'static str = "OB Scenes Remote";
    const VENDOR: &'static str = "Overbridge Scenes";
    const URL: &'static str = "https://github.com/overbridge-host";
    const EMAIL: &'static str = "";

    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: None,
        main_output_channels: None,
        ..AudioIOLayout::const_default()
    }];

    const MIDI_INPUT: MidiConfig = MidiConfig::None;
    const MIDI_OUTPUT: MidiConfig = MidiConfig::None;
    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        _buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        let port = self.params.host_port.value() as u16;
        let _ = self.worker_tx.send(WorkerMsg::Ping { port });
        true
    }

    fn process(
        &mut self,
        _buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let pos = self.params.crossfader.smoothed.next();
        if !pos.is_nan() && (pos - self.last_sent_pos).abs() > 1e-5 {
            self.last_sent_pos = pos;
            let port = self.params.host_port.value() as u16;
            let _ = self.worker_tx.send(WorkerMsg::Apply { port, pos });
        }
        ProcessStatus::KeepAlive
    }
}

impl Default for ObRemoteParams {
    fn default() -> Self {
        Self {
            crossfader: FloatParam::new(
                "Crossfader",
                0.0,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_unit("%")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),
            host_port: IntParam::new(
                "Host port",
                i32::from(DEFAULT_PORT),
                IntRange::Linear {
                    min: 1024,
                    max: 65535,
                },
            ),
        }
    }
}

impl Vst3Plugin for ObRemoteVst {
    const VST3_CLASS_ID: [u8; 16] = *b"OBScenesRemote01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Tools];
}

nih_export_vst3!(ObRemoteVst);
