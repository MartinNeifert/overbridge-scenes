//! Cross-platform parameter/preset sync pump (fingerprint polling + burst scan).
//!
//! On macOS the AppKit editor pump calls into this; elsewhere `PluginHost` ticks
//! it directly from the main run loop.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{Receiver, TryRecvError};
use parking_lot::{Mutex, RwLock};
use truce_rack_vst3::{clear_hardware_edits, hardware_edit_active, host_edit_active};

use crate::host::param_sync::{
    log_refresh_outcome, plugin_state_fingerprint, sync_params_from_plugin, ParamWsUpdate,
};
use crate::host::plugin_backend::SharedPlugin;
use crate::host::plugin_host::ParameterSnapshot;

/// ~2 s of 4 ms ticks — mirrors `editor_macos.rs`.
const STATE_REFRESH_BURST_TICKS: u32 = 500;
/// Routine full param scan cadence (in 4 ms ticks).
const SYNC_INTERVAL_TICKS: u32 = 25;
const BURST_SYNC_STRIDE: u32 = 8;

pub struct ParamSyncPump {
    plugin: SharedPlugin,
    parameters: Arc<RwLock<Vec<ParameterSnapshot>>>,
    pending_ws: Arc<Mutex<Vec<ParamWsUpdate>>>,
    param_epoch: Arc<AtomicU64>,
    param_flush: Receiver<()>,
    param_refresh: Receiver<()>,
    last_state_fingerprint: AtomicU64,
    tick_count: AtomicU32,
    pending_state_refreshes: AtomicU32,
    burst_changed: AtomicU32,
}

impl ParamSyncPump {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        plugin: SharedPlugin,
        parameters: Arc<RwLock<Vec<ParameterSnapshot>>>,
        pending_ws: Arc<Mutex<Vec<ParamWsUpdate>>>,
        param_epoch: Arc<AtomicU64>,
        param_flush: Receiver<()>,
        param_refresh: Receiver<()>,
    ) -> Self {
        Self {
            plugin,
            parameters,
            pending_ws,
            param_epoch,
            param_flush,
            param_refresh,
            last_state_fingerprint: AtomicU64::new(0),
            tick_count: AtomicU32::new(0),
            pending_state_refreshes: AtomicU32::new(0),
            burst_changed: AtomicU32::new(0),
        }
    }

    /// Run one 4 ms sync tick. `pre_scan` runs while the plugin lock is held
    /// before fingerprint detection (macOS uses this for `editor.on_idle()`).
    pub fn tick<F>(&self, pre_scan: F)
    where
        F: FnOnce(&SharedPlugin),
    {
        let mut flushed = self.param_flush.try_recv().is_ok();
        while self.param_flush.try_recv().is_ok() {
            flushed = true;
        }

        let mut force_sync = flushed;
        if self.param_refresh.try_recv().is_ok() {
            force_sync = true;
        }

        let pending = self.pending_state_refreshes.load(Ordering::Relaxed);
        let in_burst = pending > 0;
        if in_burst {
            self.pending_state_refreshes
                .store(pending - 1, Ordering::Relaxed);
            if pending % BURST_SYNC_STRIDE == 0 || pending == 1 {
                force_sync = true;
            }
        }

        let tick = self.tick_count.fetch_add(1, Ordering::Relaxed) + 1;
        let mut armed_burst = false;

        pre_scan(&self.plugin);

        if tick % 25 == 0 {
            if let Some(mut guard) = self.plugin.try_lock() {
                if let Some(fp) = plugin_state_fingerprint(&guard) {
                    let prev = self.last_state_fingerprint.load(Ordering::Relaxed);
                    if prev != fp {
                        if prev != 0 {
                            if hardware_edit_active(Duration::from_millis(800))
                                || host_edit_active(Duration::from_millis(800))
                            {
                                // Recent edit — not a preset load.
                            } else {
                                tracing::info!(
                                    "Plugin component state changed (preset/settings load) — pushing state to controller"
                                );
                                clear_hardware_edits();
                                if let Ok(bytes) = guard.save_state() {
                                    let status =
                                        guard.push_component_state_to_controller(&bytes);
                                    tracing::debug!(
                                        "setComponentState returned {status} ({} bytes)",
                                        bytes.len()
                                    );
                                }
                                self.burst_changed.store(0, Ordering::Relaxed);
                                self.pending_state_refreshes.store(
                                    STATE_REFRESH_BURST_TICKS,
                                    Ordering::Relaxed,
                                );
                                force_sync = true;
                                armed_burst = true;
                            }
                        }
                        self.last_state_fingerprint.store(fp, Ordering::Relaxed);
                    }
                }
            }
        }

        let do_scan = force_sync || armed_burst || tick % SYNC_INTERVAL_TICKS == 0;
        if do_scan {
            if let Some(mut guard) = self.plugin.try_lock() {
                let changed = sync_params_from_plugin(
                    &mut guard,
                    &self.parameters,
                    force_sync,
                    Some(&self.pending_ws),
                );
                if changed > 0 {
                    self.param_epoch.fetch_add(1, Ordering::Relaxed);
                }
                if in_burst || armed_burst {
                    let total = self
                        .burst_changed
                        .fetch_add(changed as u32, Ordering::Relaxed)
                        + changed as u32;
                    if in_burst && pending == 1 {
                        log_refresh_outcome(total as usize, true);
                    }
                }
            }
        }
    }

    pub fn drain_refresh_channel(&self) {
        while self.param_refresh.try_recv().is_ok() {}
    }
}

impl ParamSyncPump {
    pub fn noop_pre_scan(_plugin: &SharedPlugin) {}
}

/// Helper for tests: spin the pump until `param_epoch` increases or timeout.
pub fn wait_for_param_epoch(
    pump: &ParamSyncPump,
    epoch: &AtomicU64,
    before: u64,
    timeout: Duration,
) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        pump.tick(ParamSyncPump::noop_pre_scan);
        if epoch.load(Ordering::Relaxed) > before {
            return true;
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    false
}

/// Drain a channel without blocking.
pub fn drain_channel(rx: &Receiver<()>) {
    loop {
        match rx.try_recv() {
            Ok(()) => {}
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
        }
    }
}
