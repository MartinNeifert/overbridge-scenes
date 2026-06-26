//! Overbridge Host library — VST host with programmatic control API.

pub mod api;
pub mod bin_main;
pub mod config;
pub mod crossfader;
pub mod devices;
pub mod engine;
pub mod host;
pub mod match_devices;
pub mod midi;
pub mod net_util;
pub mod scenes_store;
pub mod state;

#[cfg(test)]
pub mod test_support;

#[cfg(test)]
mod testing;

pub use config::AppConfig;
pub use host::PluginHost;
pub use state::AppState;
