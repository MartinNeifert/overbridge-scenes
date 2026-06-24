//! Control worker thread — parameter/MIDI command dispatch without audio I/O.

use anyhow::Result;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use parking_lot::RwLock;
use std::sync::Arc;
use truce_rack_core::events::{Event, EventBody, MidiData};

use crate::host::param_sync::{sync_params_from_plugin, update_param_snapshot};
use crate::host::plugin_backend::{PluginInstance, SharedPlugin};
use crate::host::plugin_host::{HostCommand, ParameterSnapshot};

pub struct ControlEngine;

impl ControlEngine {
    pub fn run(
        plugin: SharedPlugin,
        parameters: Arc<RwLock<Vec<ParameterSnapshot>>>,
        cmd_rx: Receiver<HostCommand>,
        shutdown_rx: Receiver<()>,
        param_flush: Sender<()>,
    ) -> Result<()> {
        tracing::info!(
            "Control-only host: no audio device, no process(); \
             parameters via edit controller — hardware audio untouched"
        );

        // MIDI would normally be queued for process(); without process() it is dropped.
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
                p.clear_pending_param_changes();
            }
            event_sink.lock().clear();
        }

        Ok(())
    }
}

pub(crate) fn apply_command(
    plugin: &mut PluginInstance,
    parameters: &Arc<RwLock<Vec<ParameterSnapshot>>>,
    cmd: HostCommand,
    pending_events: &Arc<parking_lot::Mutex<Vec<Event>>>,
    param_flush: &Sender<()>,
) {
    match cmd {
        HostCommand::SetParameterByName { name, value } => {
            let idx = parameters
                .read()
                .iter()
                .position(|p| p.name.eq_ignore_ascii_case(&name));
            if let Some(index) = idx {
                if plugin.set_parameter(index, value).is_ok() {
                    update_param_snapshot(plugin, parameters, index);
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
            tracing::debug!("macro apply not yet implemented in control thread");
        }
        HostCommand::SyncAllParameters => {
            sync_params_from_plugin(plugin, parameters, true, None);
        }
    }
}
