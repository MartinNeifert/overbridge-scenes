pub mod control;
#[cfg(target_os = "macos")]
pub mod editor_macos;
pub mod fake_plugin;
#[cfg(target_os = "macos")]
pub mod gui_macos;
pub mod param_sync;
pub mod param_sync_pump;
pub mod plugin_backend;
pub mod plugin_host;
pub mod test_params;

pub use fake_plugin::FakePlugin;
pub use plugin_backend::{PluginInstance, SharedPlugin};
pub use plugin_host::{HostCommand, ParameterIndex, ParameterSnapshot, PluginHost};
