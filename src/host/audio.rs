use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use cpal::traits::{DeviceTrait, StreamTrait};
use parking_lot::RwLock;
use std::sync::Arc;
use truce_rack_core::buffer::{AudioBuffer, BusRange};
use truce_rack_core::bus::{Bus, BusLayout, ChannelConfig};
use truce_rack_core::events::{Event, EventBody, EventList, MidiData};
use truce_rack_core::plugin::{Plugin, PluginCore, ProcessContext};
use truce_rack::vst3::Vst3Plugin;

use crate::host::audio_device::{find_device_by_name, OverbridgeAudioDevice};
use crate::host::param_sync::sync_params_from_plugin;
use crate::host::plugin_host::{HostCommand, ParameterSnapshot, SharedPlugin};

pub struct AudioEngine;

/// Runtime configuration for the native CoreAudio duplex path.
#[derive(Clone, Debug)]
pub struct DuplexSettings {
    /// Device name substring to open as a single duplex AUHAL.
    pub device: String,
    /// Route the device's own input back to its output (DAW-style monitoring).
    pub monitor: bool,
    /// First device INPUT channel used as the monitor source (left).
    pub monitor_source: usize,
    /// Linear gain applied to the monitored signal.
    pub monitor_gain: f32,
}

impl AudioEngine {
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        plugin: SharedPlugin,
        audio_device: Option<OverbridgeAudioDevice>,
        max_block: usize,
        cmd_rx: Receiver<HostCommand>,
        shutdown_rx: Receiver<()>,
        parameters: Arc<RwLock<Vec<ParameterSnapshot>>>,
        audio_ready: Sender<()>,
        param_flush: Sender<()>,
        control_only: bool,
        monitor: bool,
        passthru: bool,
        duplex: Option<DuplexSettings>,
    ) -> Result<()> {
        // Control-only: never engage the audio engine at all — don't open the
        // Overbridge device, don't `setActive`/`setProcessing`, and never call
        // `process()`. Engaging the VST audio processor (or opening the device's
        // output stream) makes the hardware drop its own audio. Parameter changes
        // are delivered purely through the edit controller (the same path the
        // plugin GUI uses), driven by the editor run-loop pump.
        // See docs/designs/audio-routing-and-control-options.md.
        if control_only {
            let _ = audio_ready.send(());
            return Self::run_controller_only(
                plugin,
                parameters,
                cmd_rx,
                shutdown_rx,
                param_flush,
            );
        }

        // Native CoreAudio duplex (opt-in via OB_CA_DUPLEX): open a SINGLE AUHAL
        // AudioUnit on the Elektron device with both input and output enabled and
        // drive process() from its one render callback. This is what a DAW does
        // when you select the device as its audio interface — one device, one
        // clock — which is the only configuration whose round-trip latency the
        // Overbridge Engine can measure without faulting. cpal cannot do this
        // (it opens separate, unsynchronised input/output units).
        #[cfg(target_os = "macos")]
        {
            // Prefer explicit settings; fall back to the OB_CA_DUPLEX env var so
            // the mode can still be flipped on without editing config.
            let duplex = duplex.or_else(|| {
                std::env::var("OB_CA_DUPLEX").ok().and_then(|v| {
                    if v.is_empty() {
                        return None;
                    }
                    let device = if v == "1" { "Digitakt".to_string() } else { v };
                    Some(DuplexSettings {
                        device,
                        monitor: true,
                        monitor_source: 0,
                        monitor_gain: 1.0,
                    })
                })
            });
            if let Some(settings) = duplex {
                if !settings.device.is_empty() {
                    return Self::run_coreaudio_duplex(
                        plugin,
                        settings,
                        max_block,
                        cmd_rx,
                        shutdown_rx,
                        parameters,
                        audio_ready,
                        param_flush,
                    );
                }
            }
        }

        // Pick what clocks process(). The Overbridge hardware is NOT a CoreAudio
        // device — the Engine talks to it over USB privately and feeds the VST
        // through IPC — so there is nothing "Overbridge" to open as an audio
        // device. A DAW drives the plugin's process() from whatever real output
        // device the DAW itself runs on (a rock-steady hardware clock). We mirror
        // that: when no explicit Overbridge device is passed, open the system
        // default output device purely as a CLOCK (we write silence to it). This
        // is the key fix over the old software-timer loop, whose jitter the
        // Overbridge Engine eventually rejects, faulting and cutting the device.
        let driver = match audio_device {
            Some(dev) => Driver::Overbridge(dev),
            None => match open_clock_device(max_block) {
                Ok(c) => Driver::Clock(c),
                Err(e) => {
                    tracing::warn!(
                        "No output device available to clock process() ({e:#}); \
                         falling back to software-timer loop"
                    );
                    Driver::Timer
                }
            },
        };

        // Activate at the clock's real sample rate so process() runs at the same
        // rate as the hardware clock that drives it.
        let (channels, sr) = match &driver {
            Driver::Overbridge(d) => (usize::from(d.channels.max(1)), f64::from(d.sample_rate)),
            Driver::Clock(c) => (2usize, c.sample_rate),
            Driver::Timer => (2usize, 48_000.0f64),
        };

        let layout = bus_layout_for_channels(channels);
        {
            let mut p = plugin.lock();
            tracing::info!("Activating plugin (native bus layout)...");
            p.activate(layout, sr, max_block)
                .context("activate plugin")?;
            tracing::info!("Plugin activated");
        }
        let _ = audio_ready.send(());

        let audio_device = match driver {
            Driver::Clock(clock) => {
                return Self::run_clock_device(
                    plugin,
                    clock,
                    max_block,
                    cmd_rx,
                    shutdown_rx,
                    parameters,
                    param_flush,
                    sr,
                );
            }
            Driver::Timer => {
                return Self::run_engine_continuous(
                    plugin,
                    channels,
                    sr,
                    max_block,
                    cmd_rx,
                    shutdown_rx,
                    parameters,
                    param_flush,
                );
            }
            Driver::Overbridge(dev) => dev,
        };
        // Default: open only the device's INPUT stream and drive process() from
        // it. A DAW reads the Overbridge device's channels via a single stream;
        // opening a second (output) cpal stream on the same device appears to be
        // what the Overbridge driver rejects. --audio/--passthru still open the
        // duplex (input + output) path below.
        if !monitor && !passthru {
            return Self::run_input_only(
                plugin,
                audio_device,
                max_block,
                cmd_rx,
                shutdown_rx,
                parameters,
                param_flush,
                sr,
            );
        }

        // Experiment: open ONLY the device's OUTPUT stream as a single CoreAudio
        // AudioUnit and clock process() from it, writing the plugin's output to
        // the device. This is the closest cpal can get to a DAW's "output device
        // = Digitakt" (one output unit on the device), without opening a second
        // input unit that runs on an independent clock.
        if monitor
            && std::env::var("OB_OUT_ONLY")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false)
        {
            return Self::run_output_only(
                plugin,
                audio_device,
                max_block,
                cmd_rx,
                shutdown_rx,
                parameters,
                param_flush,
                sr,
            );
        }

        let channels = usize::from(audio_device.channels.max(1));
        let sample_rate = audio_device.sample_rate;
        let stream_config = audio_device.stream_config.clone();

        let mut input_buf = vec![vec![0.0f32; max_block]; channels];
        let mut output_buf = vec![vec![0.0f32; max_block]; channels];
        let bus_in = vec![BusRange::new(0, channels)];
        let bus_out = vec![BusRange::new(0, channels)];

        let pending_events: Arc<parking_lot::Mutex<Vec<Event>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));

        let events_for_cb = Arc::clone(&pending_events);
        let cmd_rx_audio = cmd_rx.clone();
        let parameters_audio = Arc::clone(&parameters);
        let param_flush_audio = param_flush.clone();

        let input_device =
            find_device_by_name(&audio_device.name).context("re-open Overbridge input device")?;
        let input_channels = channels;
        let captured_input: Arc<parking_lot::Mutex<Vec<Vec<f32>>>> =
            Arc::new(parking_lot::Mutex::new(vec![vec![0.0f32; max_block]; channels]));
        let captured_for_input = Arc::clone(&captured_input);

        let mut input_stream_config = stream_config.clone();
        if let Ok(input_configs) = input_device.supported_input_configs() {
            let configs: Vec<_> = input_configs
                .filter(|c| {
                    c.min_sample_rate().0 <= sample_rate && c.max_sample_rate().0 >= sample_rate
                })
                .map(|c| c.with_sample_rate(cpal::SampleRate(sample_rate)))
                .collect();
            if let Some(best) = configs
                .iter()
                .max_by_key(|c| c.channels())
                .cloned()
            {
                input_stream_config = best.config();
                input_stream_config.sample_rate = cpal::SampleRate(sample_rate);
                input_stream_config.buffer_size = cpal::BufferSize::Fixed(max_block as u32);
            }
        }

        let input_ch = usize::from(input_stream_config.channels.max(1));
        let input_stream = input_device
            .build_input_stream(
                &input_stream_config,
                move |data: &[f32], _| {
                    let ch = input_channels.max(1);
                    let frames = data.len() / input_ch.max(1);
                    let mut cap = captured_for_input.lock();
                    for c in cap.iter_mut() {
                        if c.len() < frames {
                            c.resize(frames, 0.0);
                        }
                    }
                    for frame in 0..frames {
                        for c in 0..ch.min(cap.len()) {
                            cap[c][frame] = data[frame * input_ch + c.min(input_ch - 1)];
                        }
                    }
                },
                |err| tracing::error!("cpal input stream error: {err}"),
                None,
            )
            .context("build Overbridge input stream")?;

        let captured_for_output = Arc::clone(&captured_input);
        let plugin_cb = Arc::clone(&plugin);
        let stream = audio_device
            .device
            .build_output_stream(
                &stream_config,
                move |out: &mut [f32], _| {
                    let frames = out.len() / channels.max(1);

                    // Real-time discipline: never block the audio thread on the
                    // plugin lock (the editor / param-sync pump holds it for slow
                    // work). If we can't get it this buffer, emit the fallback
                    // output and return so CoreAudio never misses its deadline —
                    // that deadline-miss is what makes Overbridge drop the device.
                    let Some(mut p) = plugin_cb.try_lock() else {
                        if passthru {
                            let cap = captured_for_output.lock();
                            for frame in 0..frames {
                                for ch in 0..channels {
                                    out[frame * channels + ch] =
                                        cap.get(ch).and_then(|c| c.get(frame)).copied().unwrap_or(0.0);
                                }
                            }
                        } else {
                            out.fill(0.0);
                        }
                        return;
                    };

                    while let Ok(cmd) = cmd_rx_audio.try_recv() {
                        apply_command(
                            &mut p,
                            &parameters_audio,
                            cmd,
                            &events_for_cb,
                            &param_flush_audio,
                        );
                    }

                    {
                        let cap = captured_for_output.lock();
                        for c in 0..channels {
                            if c < cap.len() && cap[c].len() >= frames {
                                input_buf[c][..frames].copy_from_slice(&cap[c][..frames]);
                            } else {
                                input_buf[c][..frames].fill(0.0);
                            }
                        }
                    }

                    for ch in &mut output_buf {
                        if ch.len() < frames {
                            ch.resize(frames, 0.0);
                        }
                        ch[..frames].fill(0.0);
                    }

                    let inputs: Vec<&[f32]> = input_buf.iter().map(|c| &c[..frames]).collect();
                    let mut outputs: Vec<&mut [f32]> =
                        output_buf.iter_mut().map(|c| &mut c[..frames]).collect();

                    let mut buffer =
                        AudioBuffer::new(&inputs, &mut outputs, frames, &bus_in, &bus_out);

                    let mut events = EventList::default();
                    {
                        if let Some(mut pending) = events_for_cb.try_lock() {
                            for event in pending.drain(..) {
                                events.push(event);
                            }
                        }
                    }

                    let mut out_events = EventList::default();
                    let mut ctx = ProcessContext {
                        sample_rate: sr,
                        max_block_size: max_block,
                        transport: None,
                        output_events: &mut out_events,
                    };

                    let _ = p.process(&mut buffer, &events, &mut ctx);

                    // Output routing:
                    //   passthru — loop the device's captured input straight back
                    //              to its output, so the hardware keeps playing its
                    //              own audio while the host stays connected.
                    //   monitor  — send the plugin's processed output to the device.
                    //   else     — silence.
                    if passthru {
                        for frame in 0..frames {
                            for ch in 0..channels {
                                out[frame * channels + ch] =
                                    input_buf[ch.min(input_buf.len() - 1)][frame];
                            }
                        }
                    } else if monitor {
                        for frame in 0..frames {
                            for ch in 0..channels {
                                out[frame * channels + ch] =
                                    output_buf[ch.min(output_buf.len() - 1)][frame];
                            }
                        }
                    } else {
                        out.fill(0.0);
                    }
                },
                |err| tracing::error!("cpal output stream error: {err}"),
                None,
            )
            .context("build Overbridge output stream")?;

        input_stream.play().context("start input stream")?;
        stream.play().context("start output stream")?;
        tracing::info!(
            "Audio running on \"{}\": {} Hz, {} ch duplex, block {} ({})",
            audio_device.name,
            sample_rate,
            channels,
            max_block,
            if passthru {
                "passthru: device input → device output"
            } else if monitor {
                "monitor: plugin output → device"
            } else {
                "silent output → device audio untouched"
            }
        );

        loop {
            match shutdown_rx.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {}
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        drop(stream);
        drop(input_stream);
        Ok(())
    }

    /// Controller-only loop: the audio engine is never engaged. No audio device
    /// is opened, the plugin is never activated (`setActive`/`setProcessing`),
    /// and `process()` is never called — so the Overbridge hardware keeps its own
    /// audio untouched.
    ///
    /// Parameter changes are delivered through `IEditController::setParamNormalized`
    /// (inside `Vst3Plugin::set_parameter`), the same path the plugin GUI uses
    /// when a knob is turned. The editor run-loop pump (started by `PluginHost`)
    /// services the plugin's device IPC so those edits reach the hardware.
    ///
    /// MIDI note/CC commands require `process()` to reach the device and are
    /// therefore dropped here; scene/crossfader control is parameter-based.
    fn run_controller_only(
        plugin: SharedPlugin,
        parameters: Arc<RwLock<Vec<ParameterSnapshot>>>,
        cmd_rx: Receiver<HostCommand>,
        shutdown_rx: Receiver<()>,
        param_flush: Sender<()>,
    ) -> Result<()> {
        tracing::info!(
            "Controller-only mode: audio engine NOT engaged (no device, no process()); \
             parameters delivered via the edit controller — hardware audio untouched"
        );

        // MIDI events would normally be queued here for process(); with no
        // process() they are collected and discarded.
        let event_sink: Arc<parking_lot::Mutex<Vec<Event>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));

        loop {
            match shutdown_rx.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {}
            }

            let first = match cmd_rx.recv_timeout(std::time::Duration::from_millis(150)) {
                Ok(cmd) => cmd,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            };

            {
                let mut p = plugin.lock();
                apply_command(&mut p, &parameters, first, &event_sink, &param_flush);
                while let Ok(cmd) = cmd_rx.try_recv() {
                    apply_command(&mut p, &parameters, cmd, &event_sink, &param_flush);
                }
                // Never processed → drop queued IParameterChanges / MIDI so
                // memory stays flat.
                p.clear_pending_param_changes();
            }
            event_sink.lock().clear();
        }

        Ok(())
    }

    /// Native CoreAudio duplex: activate the plugin and run a single duplex AUHAL
    /// on the Elektron device (see `coreaudio_duplex`). The device's own clock
    /// drives `process()`, with input and output on one coherent stream so the
    /// Overbridge Engine's round-trip latency measurement completes — exactly
    /// like a DAW with the device selected as its audio interface.
    #[cfg(target_os = "macos")]
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn run_coreaudio_duplex(
        plugin: SharedPlugin,
        settings: DuplexSettings,
        max_block: usize,
        cmd_rx: Receiver<HostCommand>,
        shutdown_rx: Receiver<()>,
        parameters: Arc<RwLock<Vec<ParameterSnapshot>>>,
        audio_ready: Sender<()>,
        param_flush: Sender<()>,
    ) -> Result<()> {
        let sr = 48_000.0f64;
        {
            let mut p = plugin.lock();
            tracing::info!("Activating plugin (native bus layout) for CoreAudio duplex...");
            p.activate(bus_layout_for_channels(2), sr, max_block)
                .context("activate plugin")?;
            tracing::info!("Plugin activated");
        }
        let _ = audio_ready.send(());

        let stream = crate::host::coreaudio_duplex::DuplexStream::open(
            &settings.device,
            sr,
            max_block,
            Arc::clone(&plugin),
            cmd_rx,
            Arc::clone(&parameters),
            param_flush,
            crate::host::coreaudio_duplex::MonitorConfig {
                enabled: settings.monitor,
                source: settings.monitor_source,
                gain: settings.monitor_gain,
            },
        )
        .context("open CoreAudio duplex stream")?;

        tracing::info!(
            "CoreAudio duplex mode: single AUHAL on \"{}\" — {} in / {} out @ {} Hz, block {} (monitor {}, source ch {}, gain {:.2}; one device, one clock, DAW-equivalent)",
            stream.device_name,
            stream.in_channels,
            stream.out_channels,
            sr as u32,
            max_block,
            if settings.monitor { "on" } else { "off" },
            settings.monitor_source,
            settings.monitor_gain,
        );

        let mut last_log = std::time::Instant::now();
        let mut last_callbacks = 0u64;
        loop {
            match shutdown_rx.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {}
            }
            std::thread::sleep(std::time::Duration::from_millis(50));

            if last_log.elapsed() >= std::time::Duration::from_secs(2) {
                use std::sync::atomic::Ordering;
                let cb = stream.stats.callbacks.load(Ordering::Relaxed);
                let errs = stream.stats.input_render_errors.load(Ordering::Relaxed);
                let st = stream.stats.last_input_render_status.load(Ordering::Relaxed);
                let in_peak =
                    stream.stats.input_peak_micros.swap(0, Ordering::Relaxed) as f64 / 1.0e6;
                let dt = last_log.elapsed().as_secs_f64();
                let rate = ((cb - last_callbacks) as f64 / dt) as u64;
                tracing::info!(
                    "duplex: {rate} callbacks/s (total {cb}), input-render errors {errs} (last status {st}), device-in peak {in_peak:.4}"
                );
                last_callbacks = cb;
                last_log = std::time::Instant::now();
            }
        }

        drop(stream);
        Ok(())
    }

    /// Default engine loop: drive `process()` continuously at real-time cadence
    /// without opening any audio device. The plugin is hosted at its full
    /// multibus layout (see `Vst3Plugin::activate`), so the Overbridge Engine
    /// keeps streaming the device and the hardware keeps its own audio.
    /// Parameter / MIDI changes are applied between blocks.
    fn run_engine_continuous(
        plugin: SharedPlugin,
        channels: usize,
        sr: f64,
        max_block: usize,
        cmd_rx: Receiver<HostCommand>,
        shutdown_rx: Receiver<()>,
        parameters: Arc<RwLock<Vec<ParameterSnapshot>>>,
        param_flush: Sender<()>,
    ) -> Result<()> {
        let frames = max_block;
        let ch = channels.max(1);
        let mut input_buf = vec![vec![0.0f32; frames]; ch];
        let mut output_buf = vec![vec![0.0f32; frames]; ch];
        let bus_in = vec![BusRange::new(0, ch)];
        let bus_out = vec![BusRange::new(0, ch)];
        let pending_events: Arc<parking_lot::Mutex<Vec<Event>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));
        let block_dur = std::time::Duration::from_secs_f64((frames as f64 / sr).max(0.0005));

        tracing::info!(
            "Engine mode: continuous multibus process() at {} Hz, block {} (no audio device opened; device audio streamed by the Engine)",
            sr as u32,
            frames
        );

        loop {
            match shutdown_rx.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {}
            }

            {
                let mut p = plugin.lock();
                while let Ok(cmd) = cmd_rx.try_recv() {
                    apply_command(&mut p, &parameters, cmd, &pending_events, &param_flush);
                }

                for c in input_buf.iter_mut() {
                    c[..frames].fill(0.0);
                }
                for c in output_buf.iter_mut() {
                    c[..frames].fill(0.0);
                }
                let inputs: Vec<&[f32]> = input_buf.iter().map(|c| &c[..frames]).collect();
                let mut outputs: Vec<&mut [f32]> =
                    output_buf.iter_mut().map(|c| &mut c[..frames]).collect();
                let mut buffer = AudioBuffer::new(&inputs, &mut outputs, frames, &bus_in, &bus_out);

                let mut events = EventList::default();
                {
                    let mut pending = pending_events.lock();
                    for event in pending.drain(..) {
                        events.push(event);
                    }
                }
                let mut out_events = EventList::default();
                let mut ctx = ProcessContext {
                    sample_rate: sr,
                    max_block_size: max_block,
                    transport: None,
                    output_events: &mut out_events,
                };
                let _ = p.process(&mut buffer, &events, &mut ctx);
            }

            std::thread::sleep(block_dur);
        }

        Ok(())
    }

    /// Clock-driven engine: drive `process()` from a real output device's
    /// CoreAudio callback (a steady hardware clock), writing silence to that
    /// device. This is how a DAW hosts the Overbridge plugin — the plugin runs
    /// on the DAW's audio device clock, not a software timer — and is what keeps
    /// the Overbridge Engine streaming the hardware without faulting.
    fn run_clock_device(
        plugin: SharedPlugin,
        clock: ClockDevice,
        max_block: usize,
        cmd_rx: Receiver<HostCommand>,
        shutdown_rx: Receiver<()>,
        parameters: Arc<RwLock<Vec<ParameterSnapshot>>>,
        param_flush: Sender<()>,
        sr: f64,
    ) -> Result<()> {
        let frames_max = max_block;
        let out_ch = clock.out_channels.max(1);
        let bus_in = vec![BusRange::new(0, 1)];
        let bus_out = vec![BusRange::new(0, 1)];
        let mut dummy_in = vec![0.0f32; frames_max];
        let mut dummy_out = vec![0.0f32; frames_max];
        let pending_events: Arc<parking_lot::Mutex<Vec<Event>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));

        let stream = clock
            .device
            .build_output_stream(
                &clock.config,
                move |out: &mut [f32], _| {
                    let frames = (out.len() / out_ch).clamp(1, frames_max);
                    // We route no audio to this device — it is purely a clock.
                    out.fill(0.0);

                    // Real-time discipline: never block on the plugin lock (the
                    // editor / param-sync pump holds it for slow work). Skipping a
                    // buffer is harmless; pending param changes persist and are
                    // delivered on the next acquired buffer.
                    let Some(mut p) = plugin.try_lock() else {
                        return;
                    };
                    while let Ok(cmd) = cmd_rx.try_recv() {
                        apply_command(&mut p, &parameters, cmd, &pending_events, &param_flush);
                    }

                    dummy_in[..frames].fill(0.0);
                    dummy_out[..frames].fill(0.0);
                    let inputs: Vec<&[f32]> = vec![&dummy_in[..frames]];
                    let mut outputs: Vec<&mut [f32]> = vec![&mut dummy_out[..frames]];
                    let mut buffer =
                        AudioBuffer::new(&inputs, &mut outputs, frames, &bus_in, &bus_out);

                    let mut events = EventList::default();
                    if let Some(mut pend) = pending_events.try_lock() {
                        for e in pend.drain(..) {
                            events.push(e);
                        }
                    }
                    let mut out_events = EventList::default();
                    let mut ctx = ProcessContext {
                        sample_rate: sr,
                        max_block_size: max_block,
                        transport: None,
                        output_events: &mut out_events,
                    };
                    let _ = p.process(&mut buffer, &events, &mut ctx);
                },
                |err| tracing::error!("cpal clock stream error: {err}"),
                None,
            )
            .context("build clock output stream")?;

        stream.play().context("start clock stream")?;
        tracing::info!(
            "Clock mode: driving multibus process() from output device \"{}\" at {} Hz, block {} (silent; DAW-style steady hardware clock)",
            clock.name,
            clock.sample_rate as u32,
            max_block
        );

        loop {
            match shutdown_rx.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {}
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        drop(stream);
        Ok(())
    }

    /// Input-only loop: open just the device's input stream (the device's audio
    /// channels) and drive `process()` from its real-time callback. This mirrors
    /// how a DAW reads an Overbridge device through a single stream; opening a
    /// second output stream on the same device appears to make the Overbridge
    /// driver drop the device's audio. The plugin's multibus output is processed
    /// and discarded — we run only to keep the Engine streaming and deliver
    /// parameter changes.
    fn run_input_only(
        plugin: SharedPlugin,
        audio_device: OverbridgeAudioDevice,
        max_block: usize,
        cmd_rx: Receiver<HostCommand>,
        shutdown_rx: Receiver<()>,
        parameters: Arc<RwLock<Vec<ParameterSnapshot>>>,
        param_flush: Sender<()>,
        sr: f64,
    ) -> Result<()> {
        let sample_rate = audio_device.sample_rate;
        let input_device =
            find_device_by_name(&audio_device.name).context("open Overbridge input device")?;

        let mut cfg = audio_device.stream_config.clone();
        if let Ok(configs) = input_device.supported_input_configs() {
            if let Some(best) = configs
                .filter(|c| {
                    c.min_sample_rate().0 <= sample_rate && c.max_sample_rate().0 >= sample_rate
                })
                .map(|c| c.with_sample_rate(cpal::SampleRate(sample_rate)))
                .max_by_key(cpal::SupportedStreamConfig::channels)
            {
                cfg = best.config();
                cfg.sample_rate = cpal::SampleRate(sample_rate);
                cfg.buffer_size = cpal::BufferSize::Fixed(max_block as u32);
            }
        }
        let in_ch = usize::from(cfg.channels.max(1));

        let pending_events: Arc<parking_lot::Mutex<Vec<Event>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));
        let bus_in = vec![BusRange::new(0, 1)];
        let bus_out = vec![BusRange::new(0, 1)];
        let mut dummy_in = vec![0.0f32; max_block];
        let mut dummy_out = vec![0.0f32; max_block];

        let stream = input_device
            .build_input_stream(
                &cfg,
                move |data: &[f32], _| {
                    let frames = (data.len() / in_ch.max(1)).clamp(1, max_block);

                    // Real-time discipline (this is how DAWs avoid device dropouts):
                    // NEVER block the audio thread on the plugin lock. The editor /
                    // param-sync pump holds that same lock while doing slow work
                    // (editor on_idle, getState fingerprints, full param scans). If
                    // we block here, the CoreAudio callback misses its deadline and
                    // the Overbridge Engine drops the device audio. Instead, grab the
                    // lock opportunistically; if the UI side has it, skip process()
                    // for this buffer (output is discarded anyway) so the device
                    // stream stays serviced on time. Pending param changes persist in
                    // the plugin and get delivered on the next buffer we do acquire.
                    let Some(mut p) = plugin.try_lock() else {
                        return;
                    };
                    while let Ok(cmd) = cmd_rx.try_recv() {
                        apply_command(&mut p, &parameters, cmd, &pending_events, &param_flush);
                    }

                    dummy_in[..frames].fill(0.0);
                    dummy_out[..frames].fill(0.0);
                    let inputs: Vec<&[f32]> = vec![&dummy_in[..frames]];
                    let mut outputs: Vec<&mut [f32]> = vec![&mut dummy_out[..frames]];
                    let mut buffer =
                        AudioBuffer::new(&inputs, &mut outputs, frames, &bus_in, &bus_out);

                    let mut events = EventList::default();
                    {
                        if let Some(mut pend) = pending_events.try_lock() {
                            for e in pend.drain(..) {
                                events.push(e);
                            }
                        }
                    }
                    let mut out_events = EventList::default();
                    let mut ctx = ProcessContext {
                        sample_rate: sr,
                        max_block_size: max_block,
                        transport: None,
                        output_events: &mut out_events,
                    };
                    let _ = p.process(&mut buffer, &events, &mut ctx);
                },
                |err| tracing::error!("cpal input stream error: {err}"),
                None,
            )
            .context("build Overbridge input stream")?;

        stream.play().context("start input stream")?;
        tracing::info!(
            "Audio running on \"{}\": {} Hz, {} ch INPUT-only (consuming device stream; no output stream opened)",
            audio_device.name,
            sample_rate,
            in_ch
        );

        loop {
            match shutdown_rx.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {}
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        drop(stream);
        Ok(())
    }
}

/// What drives the plugin's `process()` calls.
enum Driver {
    /// An explicit Overbridge CoreAudio device (rare; `--audio`/`--passthru`).
    Overbridge(OverbridgeAudioDevice),
    /// A real output device used purely as a steady clock (DAW-like default).
    Clock(ClockDevice),
    /// Last-resort software timer when no output device is available.
    Timer,
}

/// A real output device opened only to provide a steady audio clock. We write
/// silence to it; its callback cadence drives `process()`.
struct ClockDevice {
    device: cpal::Device,
    name: String,
    sample_rate: f64,
    out_channels: usize,
    config: cpal::StreamConfig,
}

/// True if a device name looks like an Elektron/Overbridge audio endpoint.
/// We must NOT clock `process()` from the Elektron device itself — the
/// Overbridge Engine owns that CoreAudio device, and opening it from here
/// contends with the Engine and cuts the hardware audio. A DAW clocks from its
/// own interface instead, which is what we mirror.
fn is_elektron_device(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        "elektron",
        "overbridge",
        "digitakt",
        "digitone",
        "syntakt",
        "analog rytm",
        "analog four",
        "analog keys",
        "analog heat",
        "model:",
        "octatrack",
    ]
    .iter()
    .any(|h| n.contains(h))
}

/// Open a real output device to use as a steady clock for `process()`.
/// Prefers the system default output, but skips any Elektron/Overbridge device
/// (the Engine owns those) and falls back to the first non-Elektron output.
fn open_clock_device(max_block: usize) -> Result<ClockDevice> {
    use cpal::traits::HostTrait;
    let host = cpal::default_host();

    let default_ok = host.default_output_device().filter(|d| {
        d.name()
            .map(|n| !is_elektron_device(&n))
            .unwrap_or(false)
    });
    let device = match default_ok {
        Some(d) => d,
        None => host
            .output_devices()
            .context("enumerate output devices")?
            .find(|d| d.name().map(|n| !is_elektron_device(&n)).unwrap_or(false))
            .context("no non-Elektron output device available to clock process()")?,
    };
    let name = device.name().unwrap_or_else(|_| "default output".into());
    let supported = device
        .default_output_config()
        .context("query default output config")?;
    let sample_rate = f64::from(supported.sample_rate().0);
    let out_channels = usize::from(supported.channels().max(1));
    let mut config: cpal::StreamConfig = supported.config();
    config.buffer_size = cpal::BufferSize::Fixed(max_block as u32);
    Ok(ClockDevice {
        device,
        name,
        sample_rate,
        out_channels,
        config,
    })
}

impl AudioEngine {
    /// Open ONLY the Overbridge device's OUTPUT stream (a single CoreAudio
    /// AudioUnit) and drive multibus `process()` from its callback, writing the
    /// plugin's main output to the device. Mirrors a DAW with "output device =
    /// Digitakt" without opening a second, independently-clocked input unit.
    #[allow(clippy::too_many_arguments)]
    fn run_output_only(
        plugin: SharedPlugin,
        audio_device: OverbridgeAudioDevice,
        max_block: usize,
        cmd_rx: Receiver<HostCommand>,
        shutdown_rx: Receiver<()>,
        parameters: Arc<RwLock<Vec<ParameterSnapshot>>>,
        param_flush: Sender<()>,
        sr: f64,
    ) -> Result<()> {
        let sample_rate = audio_device.sample_rate;
        let channels = usize::from(audio_device.channels.max(1));
        let stream_config = audio_device.stream_config.clone();

        let bus_in = vec![BusRange::new(0, 1)];
        let bus_out = vec![BusRange::new(0, channels)];
        let mut dummy_in = vec![0.0f32; max_block];
        let mut output_buf = vec![vec![0.0f32; max_block]; channels];
        let pending_events: Arc<parking_lot::Mutex<Vec<Event>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));

        let stream = audio_device
            .device
            .build_output_stream(
                &stream_config,
                move |out: &mut [f32], _| {
                    let frames = (out.len() / channels.max(1)).clamp(1, max_block);
                    out.fill(0.0);

                    let Some(mut p) = plugin.try_lock() else {
                        return;
                    };
                    while let Ok(cmd) = cmd_rx.try_recv() {
                        apply_command(&mut p, &parameters, cmd, &pending_events, &param_flush);
                    }

                    dummy_in[..frames].fill(0.0);
                    for c in output_buf.iter_mut() {
                        c[..frames].fill(0.0);
                    }
                    let inputs: Vec<&[f32]> = vec![&dummy_in[..frames]];
                    let mut outputs: Vec<&mut [f32]> =
                        output_buf.iter_mut().map(|c| &mut c[..frames]).collect();
                    let mut buffer =
                        AudioBuffer::new(&inputs, &mut outputs, frames, &bus_in, &bus_out);

                    let mut events = EventList::default();
                    if let Some(mut pend) = pending_events.try_lock() {
                        for e in pend.drain(..) {
                            events.push(e);
                        }
                    }
                    let mut out_events = EventList::default();
                    let mut ctx = ProcessContext {
                        sample_rate: sr,
                        max_block_size: max_block,
                        transport: None,
                        output_events: &mut out_events,
                    };
                    let _ = p.process(&mut buffer, &events, &mut ctx);

                    for frame in 0..frames {
                        for ch in 0..channels {
                            out[frame * channels + ch] =
                                output_buf[ch.min(output_buf.len() - 1)][frame];
                        }
                    }
                },
                |err| tracing::error!("cpal output stream error: {err}"),
                None,
            )
            .context("build Overbridge output-only stream")?;

        stream.play().context("start output-only stream")?;
        tracing::info!(
            "Audio running on \"{}\": {} Hz, {} ch OUTPUT-only (single output AudioUnit; plugin output → device)",
            audio_device.name,
            sample_rate,
            channels
        );

        loop {
            match shutdown_rx.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {}
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        drop(stream);
        Ok(())
    }
}

fn bus_layout_for_channels(channels: usize) -> BusLayout {
    let ch = match channels {
        1 => ChannelConfig::Mono,
        2 => ChannelConfig::Stereo,
        n => ChannelConfig::Discrete(u32::try_from(n).unwrap_or(4)),
    };
    let mut layout = BusLayout::new();
    layout.inputs.push(Bus::main("Input", ch));
    layout.outputs.push(Bus::main("Output", ch));
    layout
}

pub(crate) fn apply_command(
    plugin: &mut Vst3Plugin,
    parameters: &Arc<RwLock<Vec<ParameterSnapshot>>>,
    cmd: HostCommand,
    pending_events: &Arc<parking_lot::Mutex<Vec<Event>>>,
    param_flush: &Sender<()>,
) {
    match cmd {
        HostCommand::SetParameter { index, value } => {
            if plugin.set_parameter(index, value).is_ok() {
                update_param_cache(plugin, parameters, index);
                let _ = param_flush.try_send(());
            }
        }
        HostCommand::SetParameterByName { name, value } => {
            let idx = parameters
                .read()
                .iter()
                .position(|p| p.name.eq_ignore_ascii_case(&name));
            if let Some(index) = idx {
                if plugin.set_parameter(index, value).is_ok() {
                    update_param_cache(plugin, parameters, index);
                    let _ = param_flush.try_send(());
                }
            }
        }
        HostCommand::SendMidiNote {
            channel,
            note,
            velocity,
            on,
        } => {
            let body = if on {
                EventBody::Midi(MidiData::NoteOn {
                    channel,
                    note,
                    velocity,
                })
            } else {
                EventBody::Midi(MidiData::NoteOff {
                    channel,
                    note,
                    velocity,
                })
            };
            pending_events.lock().push(Event {
                sample_offset: 0,
                body,
            });
        }
        HostCommand::SendMidiCc {
            channel,
            controller,
            value,
        } => {
            pending_events.lock().push(Event {
                sample_offset: 0,
                body: EventBody::Midi(MidiData::ControlChange {
                    channel,
                    controller,
                    value,
                }),
            });
        }
        HostCommand::SendRawMidi { data } => {
            if !data.is_empty() {
                let len = data.len().min(8) as u8;
                let mut raw = [0u8; 8];
                raw[..len as usize].copy_from_slice(&data[..len as usize]);
                pending_events.lock().push(Event {
                    sample_offset: 0,
                    body: EventBody::Midi(MidiData::Raw { len, data: raw }),
                });
            }
        }
        HostCommand::ApplyMacro { name: _, value: _ } => {
            tracing::debug!("macro apply not yet implemented in audio thread");
        }
        HostCommand::SyncAllParameters => {
            sync_params_from_plugin(plugin, parameters, true, None);
        }
    }
}

fn update_param_cache(
    plugin: &Vst3Plugin,
    parameters: &Arc<RwLock<Vec<ParameterSnapshot>>>,
    index: usize,
) {
    let mut params = parameters.write();
    if let Some(snap) = params.get_mut(index) {
        if let Ok(value) = plugin.parameter_value(index) {
            snap.value = value;
            snap.display = plugin
                .parameter_value_string(index, value)
                .unwrap_or_else(|_| format!("{value:.4}"));
        }
    }
}
