use anyhow::{Context, Result};
use midir::{Ignore, MidiInput, MidiInputConnection};
use tokio::sync::broadcast;

#[derive(Clone, Debug, serde::Serialize)]
pub struct MidiInputPort {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct MidiMessageEvent {
    pub port: String,
    pub data: Vec<u8>,
}

pub struct MidiMonitor {
    _connections: Vec<MidiInputConnection<()>>,
    ports: Vec<MidiInputPort>,
}

impl MidiMonitor {
    /// Attach a passive listener to every physical MIDI input (not our virtual
    /// control port). Messages are pushed to `tx` for WebSocket clients.
    pub fn start(tx: broadcast::Sender<MidiMessageEvent>) -> Result<Self> {
        let scanner = MidiInput::new("overbridge-scenes-monitor-scan")
            .context("create MIDI scanner")?;
        let port_names: Vec<String> = scanner
            .ports()
            .into_iter()
            .filter_map(|p| {
                let name = scanner.port_name(&p).ok()?;
                if should_skip_port(&name) {
                    return None;
                }
                Some(name)
            })
            .collect();

        let mut connections = Vec::new();
        let mut ports = Vec::new();

        for (i, name) in port_names.iter().enumerate() {
            ports.push(MidiInputPort {
                id: name.clone(),
                name: name.clone(),
            });

            let mut input =
                MidiInput::new(&format!("overbridge-scenes-monitor-{i}")).context("create MIDI input")?;
            input.ignore(Ignore::None);

            let port = input.ports().into_iter().find(|p| {
                input
                    .port_name(p)
                    .map(|n| n == *name)
                    .unwrap_or(false)
            });
            let Some(port) = port else {
                tracing::warn!("MIDI monitor: port '{name}' disappeared before connect");
                continue;
            };

            let tx = tx.clone();
            let port_name = name.clone();
            let conn = input
                .connect(
                    &port,
                    &format!("ob-scenes-mon-{port_name}"),
                    move |_, message, _| {
                        let _ = tx.send(MidiMessageEvent {
                            port: port_name.clone(),
                            data: message.to_vec(),
                        });
                    },
                    (),
                )
                .map_err(|e| anyhow::anyhow!("connect MIDI monitor to '{name}': {e}"))?;
            connections.push(conn);
        }

        tracing::info!(
            "MIDI monitor listening on {} input port(s)",
            connections.len()
        );
        Ok(Self {
            _connections: connections,
            ports,
        })
    }

    pub fn ports(&self) -> &[MidiInputPort] {
        &self.ports
    }
}

fn should_skip_port(name: &str) -> bool {
    name.contains("Overbridge Host")
        || name.starts_with("ob-scenes-mon-")
        || name.starts_with("ob-monitor-")
}
