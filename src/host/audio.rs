use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender, TryRecvError};
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

impl AudioEngine {
    pub fn run(
        plugin: SharedPlugin,
        audio_device: OverbridgeAudioDevice,
        max_block: usize,
        cmd_rx: Receiver<HostCommand>,
        shutdown_rx: Receiver<()>,
        parameters: Arc<RwLock<Vec<ParameterSnapshot>>>,
        audio_ready: Sender<()>,
        param_flush: Sender<()>,
    ) -> Result<()> {
        let channels = usize::from(audio_device.channels.max(1));
        let sample_rate = audio_device.sample_rate;
        let sr = f64::from(sample_rate);
        let stream_config = audio_device.stream_config.clone();

        let layout = bus_layout_for_channels(channels);
        {
            let mut p = plugin.lock();
            tracing::info!("Activating plugin...");
            p.activate(layout, sr, max_block)
                .context("activate plugin")?;
            tracing::info!("Plugin activated");
        }
        let _ = audio_ready.send(());

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
                    let mut p = plugin_cb.lock();

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
                        let mut pending = events_for_cb.lock();
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
            .context("build Overbridge output stream")?;

        input_stream.play().context("start input stream")?;
        stream.play().context("start output stream")?;
        tracing::info!(
            "Audio running on \"{}\": {} Hz, {} ch duplex, block {}",
            audio_device.name,
            sample_rate,
            channels,
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
        drop(input_stream);
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

fn apply_command(
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
