mod mapper;
mod monitor;

use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use midir::{Ignore, MidiInput, MidiInputConnection};
#[cfg(unix)]
use midir::os::unix::VirtualInput;
use std::sync::Arc;

use crate::host::{HostCommand, ParameterIndex};

pub struct MidiBridge {
    _connection: MidiInputConnection<()>,
}

impl MidiBridge {
    pub fn start(
        port_name: &str,
        cmd_tx: Sender<HostCommand>,
        config: mapper::MapperConfig,
        param_index: ParameterIndex,
    ) -> Result<Self> {
        let input = MidiInput::new("overbridge-host-midi").context("create MIDI input")?;
        let mut input = input;
        input.ignore(Ignore::None);

        let mapper = Arc::new(mapper::MidiMapper::new(config, param_index));

        let ports: Vec<_> = input.ports().into_iter().collect();
        let existing = ports.iter().find(|p| {
            input
                .port_name(p)
                .map(|n| n.contains(port_name))
                .unwrap_or(false)
        });

        let connection = if let Some(port) = existing {
            let cmd = cmd_tx.clone();
            let mapper_cb = Arc::clone(&mapper);
            input
                .connect(
                    port,
                    port_name,
                    move |_stamp, message, _| {
                        dispatch_message(&cmd, &mapper_cb, message);
                    },
                    (),
                )
                .map_err(|e| anyhow::anyhow!("connect to existing MIDI port: {e}"))?
        } else {
            tracing::info!("Creating virtual MIDI input port: {port_name}");
            let cmd = cmd_tx.clone();
            let mapper_cb = Arc::clone(&mapper);
            input
                .create_virtual(
                    port_name,
                    move |_stamp, message, _| {
                        dispatch_message(&cmd, &mapper_cb, message);
                    },
                    (),
                )
                .map_err(|e| anyhow::anyhow!("create virtual MIDI port: {e}"))?
        };

        tracing::info!("MIDI bridge active on port '{port_name}'");
        Ok(Self {
            _connection: connection,
        })
    }
}

fn dispatch_message(cmd_tx: &Sender<HostCommand>, mapper: &mapper::MidiMapper, message: &[u8]) {
    if let Some(cmd) = mapper.translate(message) {
        let _ = cmd_tx.send(cmd);
    } else if let Some(raw) = mapper.forward_raw(message) {
        let _ = cmd_tx.send(raw);
    }
}

pub use mapper::MapperConfig;
pub use monitor::{MidiInputPort, MidiMessageEvent, MidiMonitor};
