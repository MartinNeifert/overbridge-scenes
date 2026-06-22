pub mod audio;
pub mod audio_device;
#[cfg(target_os = "macos")]
pub mod coreaudio_duplex;
#[cfg(target_os = "macos")]
pub mod editor_macos;
#[cfg(target_os = "macos")]
pub mod gui_macos;
pub mod param_sync;
pub mod plugin_host;

pub use audio::DuplexSettings;
pub use audio_device::{list_output_devices, resolve_audio_device};
pub use plugin_host::{HostCommand, ParameterIndex, ParameterSnapshot, PluginHost};
