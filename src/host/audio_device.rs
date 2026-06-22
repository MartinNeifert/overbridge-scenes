use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, SampleRate, StreamConfig, SupportedStreamConfig};

/// Resolved cpal device for Overbridge audio I/O.
pub struct OverbridgeAudioDevice {
    pub device: Device,
    pub name: String,
    pub channels: u16,
    pub sample_rate: u32,
    pub stream_config: StreamConfig,
}

/// Find an output device whose name matches any of the given hints.
pub fn find_overbridge_device(hints: &[String], sample_rate: u32, block_size: usize) -> Result<OverbridgeAudioDevice> {
    let host = cpal::default_host();
    let devices: Vec<Device> = host
        .output_devices()
        .context("enumerate output devices")?
        .collect();

    let needle = hints
        .iter()
        .map(|h| h.to_ascii_lowercase())
        .filter(|h| !h.is_empty())
        .collect::<Vec<_>>();

    let picked = if needle.is_empty() {
        host.default_output_device()
    } else {
        devices.into_iter().find(|d| {
            d.name()
                .ok()
                .is_some_and(|name| device_name_matches(&name, &needle))
        })
    };

    let device = picked.context("no Overbridge audio device found — connect hardware and start Overbridge Engine")?;
    let name = device.name().unwrap_or_else(|_| "unknown".into());
    let supported = pick_best_config(&device, sample_rate)?;
    let mut stream_config: StreamConfig = supported.config();
    stream_config.sample_rate = SampleRate(sample_rate);
    stream_config.buffer_size = cpal::BufferSize::Fixed(block_size as u32);

    let channels = stream_config.channels.max(1);

    Ok(OverbridgeAudioDevice {
        device,
        name,
        channels,
        sample_rate,
        stream_config,
    })
}

fn device_name_matches(name: &str, needles: &[String]) -> bool {
    let n = name.to_ascii_lowercase();
    needles.iter().any(|needle| n.contains(needle) || needle.contains(&n))
}

/// Prefer the widest channel layout at the requested sample rate.
fn pick_best_config(device: &Device, sample_rate: u32) -> Result<SupportedStreamConfig> {
    let mut configs: Vec<SupportedStreamConfig> = device
        .supported_output_configs()
        .context("supported_output_configs")?
        .filter(|c| {
            c.min_sample_rate().0 <= sample_rate && c.max_sample_rate().0 >= sample_rate
        })
        .map(|c| c.with_sample_rate(SampleRate(sample_rate)))
        .collect();

    if configs.is_empty() {
        return device
            .default_output_config()
            .context("default_output_config");
    }

    configs.sort_by_key(|c| std::cmp::Reverse(c.channels()));
    Ok(configs[0].clone())
}

/// Build search hints from plugin name and connected hardware names.
pub fn audio_device_hints(plugin_name: &str, connected_names: &[String]) -> Vec<String> {
    let mut hints = Vec::new();

    for name in connected_names {
        hints.push(name.clone());
        for token in name.split_whitespace() {
            if token.len() >= 4 {
                hints.push(token.to_string());
            }
        }
    }

    hints.push(plugin_name.to_string());
    for token in plugin_name.split_whitespace() {
        if token.len() >= 4 && !token.eq_ignore_ascii_case("elektron") {
            hints.push(token.to_string());
        }
    }

    hints.sort();
    hints.dedup();
    hints
}

pub fn resolve_audio_device(cfg: &crate::config::AppConfig, plugin_name: &str) -> Result<OverbridgeAudioDevice> {
    let connected: Vec<String> = crate::devices::discover()
        .devices
        .into_iter()
        .map(|d| d.name)
        .collect();

    let hints = if let Some(explicit) = cfg.audio_device.as_ref() {
        vec![explicit.clone()]
    } else {
        audio_device_hints(plugin_name, &connected)
    };

    find_overbridge_device(&hints, cfg.sample_rate, cfg.block_size)
}

pub fn find_device_by_name(name: &str) -> Result<Device> {
    let host = cpal::default_host();
    host.output_devices()
        .context("enumerate output devices")?
        .find(|d| d.name().ok().is_some_and(|n| n == name))
        .context(format!("audio device \"{name}\" no longer available"))
}

pub fn list_output_devices() -> Result<Vec<String>> {
    let host = cpal::default_host();
    Ok(host
        .output_devices()
        .context("enumerate output devices")?
        .filter_map(|d| d.name().ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_name_matches_substrings() {
        let hints = vec!["analog heat".into(), "elektron analog heat mkii".into()];
        assert!(device_name_matches("Analog Heat", &hints));
        assert!(device_name_matches("Elektron Analog Heat MKII", &hints));
        assert!(!device_name_matches("MacBook Pro Speakers", &hints));
    }

    #[test]
    fn hints_from_plugin_and_device() {
        let hints = audio_device_hints(
            "Analog Heat",
            &["Elektron Analog Heat MKII".into()],
        );
        assert!(hints.iter().any(|h| h.contains("Analog")));
    }
}
