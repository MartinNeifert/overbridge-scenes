//! Overbridge Host library — VST host with programmatic control API.

pub mod api;
pub mod bin_main;
pub mod config;
pub mod devices;
pub mod engine;
pub mod host;
pub mod match_devices;
pub mod midi;
pub mod net_util;
pub mod scenes_store;
pub mod state;
pub mod test_support;

pub use config::AppConfig;
pub use host::PluginHost;
pub use state::AppState;
