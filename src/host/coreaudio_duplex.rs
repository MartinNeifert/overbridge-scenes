//! Native CoreAudio duplex stream for the Elektron/Overbridge device.
//!
//! `cpal` opens input and output as two *separate* AudioUnits that run on
//! independent IO procs / clocks. The Overbridge Engine measures round-trip
//! latency (it plays a probe to the device output and times it returning on the
//! device input); with two unsynchronised clocks that measurement never settles
//! and the Engine faults ("latency measurement failed") and cuts the hardware
//! audio after a few seconds.
//!
//! A DAW survives because selecting the Elektron device as its audio interface
//! opens a *single* duplex AUHAL AudioUnit — input and output on one device,
//! one clock, one render callback. This module replicates exactly that: one
//! `kAudioUnitSubType_HALOutput` unit bound to the device with both IO scopes
//! enabled, pulling input inside the output render callback and driving the
//! plugin's `process()` from that single coherent clock.
#![cfg(target_os = "macos")]
#![allow(non_upper_case_globals)]

use anyhow::{anyhow, bail, Context, Result};
use coreaudio_sys::*;
use crossbeam_channel::Receiver;
use parking_lot::RwLock;
use std::os::raw::c_void;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::Arc;
use truce_rack_core::buffer::{AudioBuffer, BusRange};
use truce_rack_core::events::{Event, EventList};
use truce_rack_core::plugin::{Plugin, ProcessContext};

use crate::host::audio::apply_command;
use crate::host::plugin_host::{HostCommand, ParameterSnapshot, SharedPlugin};

const OUTPUT_ELEMENT: AudioUnitElement = 0;
const INPUT_ELEMENT: AudioUnitElement = 1;

/// Live counters published from the audio thread for diagnostics.
#[derive(Default)]
pub struct DuplexStats {
    pub callbacks: AtomicU64,
    pub input_render_errors: AtomicU64,
    pub last_input_render_status: AtomicI32,
    /// Peak abs sample (×1e6) seen on the device INPUT (device→computer).
    pub input_peak_micros: AtomicU64,
}

/// How the device's audio is monitored back to its own output.
#[derive(Clone, Copy, Debug)]
pub struct MonitorConfig {
    /// Route the device's input back to its output. While an Overbridge host is
    /// connected the device's analog Main Out plays the USB return, so this is
    /// what keeps it audible.
    pub enabled: bool,
    /// First device INPUT channel used as the monitor source (left).
    pub source: usize,
    /// Linear gain applied to the monitored signal.
    pub gain: f32,
}

/// Context handed to the C render callback. Lives for the stream's lifetime.
struct CallbackCtx {
    plugin: SharedPlugin,
    cmd_rx: Receiver<HostCommand>,
    parameters: Arc<RwLock<Vec<ParameterSnapshot>>>,
    param_flush: crossbeam_channel::Sender<()>,
    pending_events: Arc<parking_lot::Mutex<Vec<Event>>>,
    unit: AudioUnit,
    in_channels: usize,
    out_channels: usize,
    max_block: usize,
    sr: f64,
    /// Interleaved input scratch: in_channels * max_block.
    input_scratch: Vec<f32>,
    /// Interleaved output scratch: out_channels * max_block (plugin Main bus).
    output_scratch: Vec<f32>,
    /// Plugin process scratch (the vendored multibus path builds its own bus
    /// buffers internally; these just satisfy the AudioBuffer API).
    dummy_in: Vec<f32>,
    dummy_out: Vec<f32>,
    /// How the device's own audio is monitored back to its output.
    monitor: MonitorConfig,
    /// Free-running sample position fed to the plugin's process context so it
    /// sees an advancing, "playing" timeline (DAW-like) and streams audio.
    play_samples: i64,
    stats: Arc<DuplexStats>,
}

/// An owned, running duplex stream. Stops and disposes the unit on drop.
pub struct DuplexStream {
    unit: AudioUnit,
    ctx: *mut CallbackCtx,
    pub device_name: String,
    pub sample_rate: f64,
    pub in_channels: usize,
    pub out_channels: usize,
    pub stats: Arc<DuplexStats>,
}

// The raw pointer is only touched on the audio thread; the struct is owned by
// the controlling thread which only starts/stops it.
unsafe impl Send for DuplexStream {}

impl Drop for DuplexStream {
    fn drop(&mut self) {
        unsafe {
            AudioOutputUnitStop(self.unit);
            AudioUnitUninitialize(self.unit);
            AudioComponentInstanceDispose(self.unit);
            if !self.ctx.is_null() {
                drop(Box::from_raw(self.ctx));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
impl DuplexStream {
    /// Open a single duplex AUHAL on the device whose name contains `hint`,
    /// and start driving `process()` from its render callback.
    pub fn open(
        hint: &str,
        preferred_sr: f64,
        max_block: usize,
        plugin: SharedPlugin,
        cmd_rx: Receiver<HostCommand>,
        parameters: Arc<RwLock<Vec<ParameterSnapshot>>>,
        param_flush: crossbeam_channel::Sender<()>,
        monitor: MonitorConfig,
    ) -> Result<Self> {
        unsafe {
            let (device_id, device_name) =
                find_device_by_name(hint).context("locate Elektron CoreAudio device")?;

            // The HAL output AudioUnit.
            let desc = AudioComponentDescription {
                componentType: kAudioUnitType_Output,
                componentSubType: kAudioUnitSubType_HALOutput,
                componentManufacturer: kAudioUnitManufacturer_Apple,
                componentFlags: 0,
                componentFlagsMask: 0,
            };
            let comp = AudioComponentFindNext(std::ptr::null_mut(), &desc);
            if comp.is_null() {
                bail!("no HAL output AudioComponent available");
            }
            let mut unit: AudioUnit = std::ptr::null_mut();
            os(AudioComponentInstanceNew(comp, &mut unit), "AudioComponentInstanceNew")?;

            // Enable input (element 1) and output (element 0) IO.
            let enable: u32 = 1;
            os(
                AudioUnitSetProperty(
                    unit,
                    kAudioOutputUnitProperty_EnableIO,
                    kAudioUnitScope_Input,
                    INPUT_ELEMENT,
                    &enable as *const _ as *const c_void,
                    std::mem::size_of::<u32>() as u32,
                ),
                "EnableIO(input)",
            )?;
            os(
                AudioUnitSetProperty(
                    unit,
                    kAudioOutputUnitProperty_EnableIO,
                    kAudioUnitScope_Output,
                    OUTPUT_ELEMENT,
                    &enable as *const _ as *const c_void,
                    std::mem::size_of::<u32>() as u32,
                ),
                "EnableIO(output)",
            )?;

            // Bind the unit to the Elektron device.
            os(
                AudioUnitSetProperty(
                    unit,
                    kAudioOutputUnitProperty_CurrentDevice,
                    kAudioUnitScope_Global,
                    OUTPUT_ELEMENT,
                    &device_id as *const _ as *const c_void,
                    std::mem::size_of::<AudioDeviceID>() as u32,
                ),
                "CurrentDevice",
            )?;

            // Channel counts straight from the device.
            let in_channels = device_channel_count(device_id, true).unwrap_or(2).max(1);
            let out_channels = device_channel_count(device_id, false).unwrap_or(2).max(1);

            // Try to set the device buffer frame size to our block.
            let frames: u32 = max_block as u32;
            let _ = AudioUnitSetProperty(
                unit,
                kAudioDevicePropertyBufferFrameSize,
                kAudioUnitScope_Global,
                0,
                &frames as *const _ as *const c_void,
                std::mem::size_of::<u32>() as u32,
            );

            // Interleaved Float32 stream formats.
            let in_fmt = asbd(preferred_sr, in_channels);
            let out_fmt = asbd(preferred_sr, out_channels);
            // Format the unit delivers device INPUT to us (output scope of input element).
            os(
                AudioUnitSetProperty(
                    unit,
                    kAudioUnitProperty_StreamFormat,
                    kAudioUnitScope_Output,
                    INPUT_ELEMENT,
                    &in_fmt as *const _ as *const c_void,
                    std::mem::size_of::<AudioStreamBasicDescription>() as u32,
                ),
                "StreamFormat(input)",
            )?;
            // Format we hand to the unit for device OUTPUT (input scope of output element).
            os(
                AudioUnitSetProperty(
                    unit,
                    kAudioUnitProperty_StreamFormat,
                    kAudioUnitScope_Input,
                    OUTPUT_ELEMENT,
                    &out_fmt as *const _ as *const c_void,
                    std::mem::size_of::<AudioStreamBasicDescription>() as u32,
                ),
                "StreamFormat(output)",
            )?;

            let stats = Arc::new(DuplexStats::default());
            let ctx = Box::new(CallbackCtx {
                plugin,
                cmd_rx,
                parameters,
                param_flush,
                pending_events: Arc::new(parking_lot::Mutex::new(Vec::new())),
                unit,
                in_channels,
                out_channels,
                max_block,
                sr: preferred_sr,
                input_scratch: vec![0.0f32; in_channels * max_block],
                output_scratch: vec![0.0f32; out_channels * max_block],
                dummy_in: vec![0.0f32; max_block],
                dummy_out: vec![0.0f32; max_block],
                monitor,
                play_samples: 0,
                stats: Arc::clone(&stats),
            });
            let ctx_ptr = Box::into_raw(ctx);

            // Output render callback drives everything.
            let cb = AURenderCallbackStruct {
                inputProc: Some(render_cb),
                inputProcRefCon: ctx_ptr as *mut c_void,
            };
            os(
                AudioUnitSetProperty(
                    unit,
                    kAudioUnitProperty_SetRenderCallback,
                    kAudioUnitScope_Input,
                    OUTPUT_ELEMENT,
                    &cb as *const _ as *const c_void,
                    std::mem::size_of::<AURenderCallbackStruct>() as u32,
                ),
                "SetRenderCallback",
            )?;

            os(AudioUnitInitialize(unit), "AudioUnitInitialize")?;
            os(AudioOutputUnitStart(unit), "AudioOutputUnitStart")?;

            Ok(DuplexStream {
                unit,
                ctx: ctx_ptr,
                device_name,
                sample_rate: preferred_sr,
                in_channels,
                out_channels,
                stats,
            })
        }
    }
}

/// The single output render callback. Pulls device input, runs `process()`, and
/// monitors the device's own audio back to its output — all on one coherent
/// device clock.
unsafe extern "C" fn render_cb(
    in_ref_con: *mut c_void,
    io_action_flags: *mut AudioUnitRenderActionFlags,
    in_time_stamp: *const AudioTimeStamp,
    _in_bus_number: u32,
    in_number_frames: u32,
    io_data: *mut AudioBufferList,
) -> OSStatus {
    {
        // Always present silence on the device output first, so even an early
        // return keeps the stream coherent.
        if !io_data.is_null() {
            let abl = &mut *io_data;
            let buffers =
                std::slice::from_raw_parts_mut(abl.mBuffers.as_mut_ptr(), abl.mNumberBuffers as usize);
            for b in buffers.iter_mut() {
                if !b.mData.is_null() {
                    std::ptr::write_bytes(b.mData as *mut u8, 0, b.mDataByteSize as usize);
                }
            }
        }

        let ctx = &mut *(in_ref_con as *mut CallbackCtx);
        ctx.stats.callbacks.fetch_add(1, Ordering::Relaxed);
        let frames = (in_number_frames as usize).min(ctx.max_block);
        if frames == 0 {
            return 0;
        }

        // Pull device input through the SAME unit (element 1) so the round-trip
        // the Engine times is coherent.
        let in_ch = ctx.in_channels;
        let needed = in_ch * frames;
        if ctx.input_scratch.len() < needed {
            ctx.input_scratch.resize(needed, 0.0);
        }
        let mut in_abl = AudioBufferList {
            mNumberBuffers: 1,
            mBuffers: [coreaudio_sys::AudioBuffer {
                mNumberChannels: in_ch as u32,
                mDataByteSize: (needed * std::mem::size_of::<f32>()) as u32,
                mData: ctx.input_scratch.as_mut_ptr() as *mut c_void,
            }],
        };
        let render_status = AudioUnitRender(
            ctx.unit,
            io_action_flags,
            in_time_stamp,
            INPUT_ELEMENT,
            in_number_frames,
            &mut in_abl,
        );
        ctx.stats
            .last_input_render_status
            .store(render_status, Ordering::Relaxed);
        if render_status != 0 {
            ctx.stats.input_render_errors.fetch_add(1, Ordering::Relaxed);
        }
        // Meter the device input (where the device's own audio arrives).
        {
            let mut peak = 0.0f32;
            for &s in ctx.input_scratch[..needed].iter() {
                let a = s.abs();
                if a > peak {
                    peak = a;
                }
            }
            let micros = (peak * 1.0e6) as u64;
            let prev = ctx.stats.input_peak_micros.load(Ordering::Relaxed);
            if micros > prev {
                ctx.stats.input_peak_micros.store(micros, Ordering::Relaxed);
            }
        }

        // Drive the plugin. Never block the audio thread on the plugin lock.
        let Some(mut p) = ctx.plugin.try_lock() else {
            return 0;
        };
        while let Ok(cmd) = ctx.cmd_rx.try_recv() {
            apply_command(
                &mut p,
                &ctx.parameters,
                cmd,
                &ctx.pending_events,
                &ctx.param_flush,
            );
        }

        ctx.dummy_in[..frames].fill(0.0);
        ctx.dummy_out[..frames].fill(0.0);
        let inputs: Vec<&[f32]> = vec![&ctx.dummy_in[..frames]];
        let mut outputs: Vec<&mut [f32]> = vec![&mut ctx.dummy_out[..frames]];
        let bus_in = [BusRange::new(0, 1)];
        let bus_out = [BusRange::new(0, 1)];
        let mut buffer = AudioBuffer::new(&inputs, &mut outputs, frames, &bus_in, &bus_out);

        let mut events = EventList::default();
        if let Some(mut pend) = ctx.pending_events.try_lock() {
            for e in pend.drain(..) {
                events.push(e);
            }
        }
        // Present a valid, advancing, "playing" transport — a DAW always does,
        // and the Overbridge plugin only streams the device's audio when it sees
        // one. Without it the plugin's output bus stays silent.
        let beats = (ctx.play_samples as f64 / ctx.sr) * (120.0 / 60.0);
        let transport = truce_rack_core::transport::TransportInfo {
            tempo_bpm: Some(120.0),
            time_signature: Some((4, 4)),
            song_position_beats: Some(beats),
            song_position_samples: Some(ctx.play_samples),
            bar_start_beats: Some(0.0),
            playing: true,
            recording: false,
            loop_active: false,
        };
        ctx.play_samples += frames as i64;

        let mut out_events = EventList::default();
        let mut pctx = ProcessContext {
            sample_rate: ctx.sr,
            max_block_size: ctx.max_block,
            transport: Some(transport),
            output_events: &mut out_events,
        };
        let _ = p.process(&mut buffer, &events, &mut pctx);
        drop(p);

        // Build the device output. While an Overbridge host is connected the
        // device's analog Main Out plays the USB *return* (host monitoring), not
        // the device's own internal mix — and the Overbridge VST's audio output
        // bus is silent in this hosting context. So to keep the analog out
        // audible we monitor the device's own audio (which arrives on the
        // CoreAudio INPUT channels) straight back to its output, exactly as a DAW
        // does by monitoring the device's tracks. With monitoring off we present
        // silence (the device output still runs, keeping the duplex clock and the
        // Engine's round-trip measurement coherent).
        let out_ch = ctx.out_channels;
        let need = out_ch * frames;
        if ctx.output_scratch.len() < need {
            ctx.output_scratch.resize(need, 0.0);
        }
        for v in ctx.output_scratch[..need].iter_mut() {
            *v = 0.0;
        }
        if ctx.monitor.enabled {
            let in_ch = ctx.in_channels.max(1);
            let gain = ctx.monitor.gain;
            let base = ctx.monitor.source;
            for f in 0..frames {
                for c in 0..out_ch {
                    let src = f * in_ch + (base + c).min(in_ch - 1);
                    let dst = f * out_ch + c;
                    ctx.output_scratch[dst] =
                        ctx.input_scratch.get(src).copied().unwrap_or(0.0) * gain;
                }
            }
        }

        if !io_data.is_null() {
            let abl = &mut *io_data;
            let buffers = std::slice::from_raw_parts_mut(
                abl.mBuffers.as_mut_ptr(),
                abl.mNumberBuffers as usize,
            );
            // Device output is interleaved Float32 in a single buffer.
            if let Some(b) = buffers.first_mut() {
                if !b.mData.is_null() {
                    let dst = std::slice::from_raw_parts_mut(
                        b.mData as *mut f32,
                        (b.mDataByteSize as usize) / std::mem::size_of::<f32>(),
                    );
                    let n = dst.len().min(need);
                    dst[..n].copy_from_slice(&ctx.output_scratch[..n]);
                }
            }
        }

        0
    }
}

/// Build an interleaved Float32 ASBD.
fn asbd(sample_rate: f64, channels: usize) -> AudioStreamBasicDescription {
    let bytes_per_frame = (channels * std::mem::size_of::<f32>()) as u32;
    AudioStreamBasicDescription {
        mSampleRate: sample_rate,
        mFormatID: kAudioFormatLinearPCM,
        mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
        mBytesPerPacket: bytes_per_frame,
        mFramesPerPacket: 1,
        mBytesPerFrame: bytes_per_frame,
        mChannelsPerFrame: channels as u32,
        mBitsPerChannel: 32,
        mReserved: 0,
    }
}

/// Find an audio device whose name contains `hint` (case-insensitive).
unsafe fn find_device_by_name(hint: &str) -> Result<(AudioDeviceID, String)> {
    let addr = AudioObjectPropertyAddress {
        mSelector: kAudioHardwarePropertyDevices,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMaster,
    };
    let mut size: u32 = 0;
    os(
        AudioObjectGetPropertyDataSize(
            kAudioObjectSystemObject,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
        ),
        "GetPropertyDataSize(devices)",
    )?;
    let count = size as usize / std::mem::size_of::<AudioDeviceID>();
    let mut ids = vec![0 as AudioDeviceID; count];
    os(
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            ids.as_mut_ptr() as *mut c_void,
        ),
        "GetPropertyData(devices)",
    )?;

    let needle = hint.to_ascii_lowercase();
    for id in ids {
        if let Some(name) = device_name(id) {
            if name.to_ascii_lowercase().contains(&needle) {
                return Ok((id, name));
            }
        }
    }
    Err(anyhow!("no CoreAudio device whose name contains \"{hint}\""))
}

/// Read a device name via the (C-string) device-name property.
unsafe fn device_name(id: AudioDeviceID) -> Option<String> {
    let addr = AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyDeviceName,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMaster,
    };
    let mut size: u32 = 0;
    if AudioObjectGetPropertyDataSize(id, &addr, 0, std::ptr::null(), &mut size) != 0 || size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    if AudioObjectGetPropertyData(
        id,
        &addr,
        0,
        std::ptr::null(),
        &mut size,
        buf.as_mut_ptr() as *mut c_void,
    ) != 0
    {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    Some(String::from_utf8_lossy(&buf[..end]).into_owned())
}

/// Count input or output channels by summing the stream-configuration buffers.
unsafe fn device_channel_count(id: AudioDeviceID, input: bool) -> Option<usize> {
    let addr = AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyStreamConfiguration,
        mScope: if input {
            kAudioObjectPropertyScopeInput
        } else {
            kAudioObjectPropertyScopeOutput
        },
        mElement: kAudioObjectPropertyElementMaster,
    };
    let mut size: u32 = 0;
    if AudioObjectGetPropertyDataSize(id, &addr, 0, std::ptr::null(), &mut size) != 0 || size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    if AudioObjectGetPropertyData(
        id,
        &addr,
        0,
        std::ptr::null(),
        &mut size,
        buf.as_mut_ptr() as *mut c_void,
    ) != 0
    {
        return None;
    }
    let abl = &*(buf.as_ptr() as *const AudioBufferList);
    let buffers =
        std::slice::from_raw_parts(abl.mBuffers.as_ptr(), abl.mNumberBuffers as usize);
    Some(buffers.iter().map(|b| b.mNumberChannels as usize).sum())
}

/// Map a non-zero OSStatus into an error.
fn os(status: OSStatus, what: &str) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(anyhow!("{what} failed: OSStatus {status}"))
    }
}
