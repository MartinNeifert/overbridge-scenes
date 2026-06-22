//! macOS: open the Overbridge editor on the main thread and pump
//! `NSRunLoop` so JUCE timers in `RemoteDeviceClient` can run.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, TryRecvError};
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSView, NSWindow, NSWindowStyleMask};
use objc2_foundation::{NSDate, NSPoint, NSRect, NSRunLoop, NSSize};
use parking_lot::{Mutex, RwLock};
use truce_rack::vst3::Vst3Plugin;
use truce_rack_core::editor::WindowHandle;
use truce_rack_core::plugin::PluginCore;

use truce_rack_vst3::{clear_hardware_edits, hardware_edit_active, host_edit_active};

use crate::host::param_sync::{log_refresh_outcome, plugin_state_fingerprint, sync_params_from_plugin};
use crate::host::plugin_host::ParameterSnapshot;

const EDITOR_WIDTH: f64 = 900.0;
const EDITOR_HEIGHT: f64 = 700.0;
const PUMP_INTERVAL: f64 = 0.001;
/// ~2 s of 4 ms ticks — Overbridge applies presets via UpdateSettingsAsync.
const STATE_REFRESH_BURST_TICKS: u32 = 500;
/// Routine full param scan cadence (in 4 ms ticks). Reading all params is a
/// heavy COM + lock op; the audio thread needs that lock at 48 kHz, so we poll
/// at ~10 Hz rather than every tick. Real-time hardware moves arrive separately
/// via `performEdit` (param_change_rx), so this is only a catch-all.
const SYNC_INTERVAL_TICKS: u32 = 25;
/// During a post-preset burst, do the (heavier, force) scan every N ticks
/// instead of every tick.
const BURST_SYNC_STRIDE: u32 = 8;

static EDITOR_OPEN: AtomicBool = AtomicBool::new(false);

/// Call once from `main()` before the tokio runtime starts.
pub fn init_appkit() {
    let mtm = MainThreadMarker::new().expect("init_appkit must run on the main thread");
    let _ = NSApplication::sharedApplication(mtm);
}

pub struct EditorPump {
    state: Arc<PumpState>,
    shutdown_tx: crossbeam_channel::Sender<()>,
}

struct PumpState {
    plugin: Arc<Mutex<Vst3Plugin>>,
    parameters: Arc<RwLock<Vec<ParameterSnapshot>>>,
    pending_ws: Arc<Mutex<Vec<(usize, f64, String)>>>,
    param_epoch: Arc<std::sync::atomic::AtomicU64>,
    shutdown: Receiver<()>,
    editor_requests: Receiver<()>,
    param_flush: Receiver<()>,
    param_refresh: Receiver<()>,
    open_on_start: bool,
    visible_on_start: bool,
    opened_on_start: AtomicBool,
    started: Instant,
    last_state_fingerprint: AtomicU64,
    tick_count: AtomicU32,
    pending_state_refreshes: AtomicU32,
    /// Total params changed across the current post-preset refresh burst.
    burst_changed: AtomicU32,
}

impl EditorPump {
    /// Prepare run-loop pumping. Call [`Self::tick`] from the main thread event loop.
    pub fn start(
        plugin: Arc<Mutex<Vst3Plugin>>,
        parameters: Arc<RwLock<Vec<ParameterSnapshot>>>,
        pending_ws: Arc<Mutex<Vec<(usize, f64, String)>>>,
        param_epoch: Arc<std::sync::atomic::AtomicU64>,
        open_on_start: bool,
        visible_on_start: bool,
        editor_requests: Receiver<()>,
        param_flush: Receiver<()>,
        param_refresh: Receiver<()>,
    ) -> Result<Self> {
        tracing::info!(
            "Main run-loop pump ready (editor on start={}, visible={}, or on plugin requestOpenEditor)",
            open_on_start,
            visible_on_start
        );

        let (shutdown_tx, shutdown_rx) = crossbeam_channel::unbounded();
        Ok(Self {
            state: Arc::new(PumpState {
                plugin,
                parameters,
                pending_ws,
                param_epoch: Arc::clone(&param_epoch),
                shutdown: shutdown_rx,
                editor_requests,
                param_flush,
                param_refresh,
                open_on_start,
                visible_on_start,
                opened_on_start: AtomicBool::new(!open_on_start),
                started: Instant::now(),
                last_state_fingerprint: AtomicU64::new(0),
                tick_count: AtomicU32::new(0),
                pending_state_refreshes: AtomicU32::new(0),
                burst_changed: AtomicU32::new(0),
            }),
            shutdown_tx,
        })
    }

    /// Pump AppKit once. Must run on the main thread (e.g. from tokio `select!`).
    pub fn tick(&self) {
        let state = &self.state;
        match state.shutdown.try_recv() {
            Ok(()) | Err(TryRecvError::Disconnected) => return,
            Err(TryRecvError::Empty) => {}
        }

        if state.open_on_start
            && !state.opened_on_start.load(Ordering::SeqCst)
            && state.started.elapsed() >= Duration::from_millis(750)
        {
            tracing::info!("Opening Overbridge editor...");
            if open_editor_inner(&state.plugin, state.visible_on_start).is_ok() {
                state.opened_on_start.store(true, Ordering::SeqCst);
            }
        }

        while state.editor_requests.try_recv().is_ok() {
            tracing::info!("Opening Overbridge editor (plugin requestOpenEditor)...");
            let _ = open_editor_inner(&state.plugin, true);
        }

        let mut flushed = state.param_flush.try_recv().is_ok();
        while state.param_flush.try_recv().is_ok() {
            flushed = true;
        }

        let mut force_sync = flushed;
        if state.param_refresh.try_recv().is_ok() {
            force_sync = true;
        }

        let pending = state.pending_state_refreshes.load(Ordering::Relaxed);
        let in_burst = pending > 0;
        if in_burst {
            state
                .pending_state_refreshes
                .store(pending - 1, Ordering::Relaxed);
            // Throttle the heavy force-read during the burst: every stride, plus
            // the final tick so the outcome log reflects the full burst.
            if pending % BURST_SYNC_STRIDE == 0 || pending == 1 {
                force_sync = true;
            }
        }

        let tick = state.tick_count.fetch_add(1, Ordering::Relaxed) + 1;
        // Set when this tick detects a preset change and arms the burst. The
        // arming tick is the one that force-reads the new values, so it must be
        // counted toward the burst total even though `in_burst` was false at the
        // top of this tick.
        let mut armed_burst = false;

        pump_main_run_loop_once(PUMP_INTERVAL);

        // Critical section A: editor idle + preset/state-change detection. Kept
        // separate from the (heavy) param scan below so each lock hold is short —
        // the audio thread takes this same lock at block rate, and a shorter hold
        // means process() / parameter delivery is skipped far less often. (Audio
        // monitoring itself no longer depends on this lock; see coreaudio_duplex.)
        if let Some(mut guard) = state.plugin.try_lock() {
            if let Some(editor) = guard.editor() {
                editor.on_idle();
                if flushed || force_sync {
                    editor.on_idle();
                    editor.on_idle();
                }
            }

            if tick % 25 == 0 {
                if let Some(fp) = plugin_state_fingerprint(&guard) {
                    let prev = state.last_state_fingerprint.load(Ordering::Relaxed);
                    if prev != fp {
                        if prev != 0 {
                            if hardware_edit_active(std::time::Duration::from_millis(800))
                                || host_edit_active(std::time::Duration::from_millis(800))
                            {
                                // Recent edit in either direction (hardware knob via
                                // performEdit, or host UI/MIDI via set_parameter) —
                                // this state change is the user's own edit, not a
                                // device preset load. Do NOT re-apply state or arm a
                                // refresh burst; that fights the edit and thrashes the
                                // plugin lock the audio thread needs.
                            } else {
                                // Preset/settings load from the device. Overbridge
                                // applies it asynchronously into the processor and
                                // does NOT emit performEdit. The documented host
                                // sequence — component.getState → controller
                                // .setComponentState — is what makes the controller
                                // refresh its parameter values so getParamNormalized
                                // returns the new preset.
                                tracing::info!(
                                    "Plugin component state changed (preset/settings load) — pushing state to controller"
                                );
                                clear_hardware_edits();
                                if let Ok(bytes) = guard.save_state() {
                                    let status = guard.push_component_state_to_controller(&bytes);
                                    tracing::debug!(
                                        "setComponentState returned {status} ({} bytes)",
                                        bytes.len()
                                    );
                                }
                                state.burst_changed.store(0, Ordering::Relaxed);
                                state.pending_state_refreshes.store(
                                    STATE_REFRESH_BURST_TICKS,
                                    Ordering::Relaxed,
                                );
                                force_sync = true;
                                armed_burst = true;
                            }
                        }
                        state.last_state_fingerprint.store(fp, Ordering::Relaxed);
                    }
                }
            }
        }

        // Only scan when due: on host edits / explicit refresh (force_sync), when
        // arming a burst, or on the routine ~10 Hz cadence. Avoids a 250 Hz
        // full-param sweep that starves the audio thread's lock.
        let do_scan = force_sync || armed_burst || tick % SYNC_INTERVAL_TICKS == 0;
        if do_scan {
            // Critical section B: the full-param scan (heavy COM + lock). Acquired
            // separately from section A so the audio thread gets a window to take
            // the lock between editor idle and this scan.
            if let Some(mut guard) = state.plugin.try_lock() {
                let changed = sync_params_from_plugin(
                    &mut guard,
                    &state.parameters,
                    force_sync,
                    Some(&state.pending_ws),
                );
                if changed > 0 {
                    state
                        .param_epoch
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                if in_burst || armed_burst {
                    let total = state
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

    pub fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
    }
}

fn open_editor_inner(plugin: &Arc<Mutex<Vst3Plugin>>, visible: bool) -> Result<()> {
    if EDITOR_OPEN.swap(true, Ordering::SeqCst) {
        tracing::debug!("Overbridge editor already open");
        return Ok(());
    }

    let mtm = MainThreadMarker::new().expect("editor open must run on the main thread");

    let app = NSApplication::sharedApplication(mtm);
    app.activateIgnoringOtherApps(true);

    let (x, y) = if visible { (100.0, 100.0) } else { (-2000.0, -2000.0) };
    let style = if visible {
        NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable
    } else {
        NSWindowStyleMask::Borderless
    };

    let rect = NSRect {
        origin: NSPoint { x, y },
        size: NSSize {
            width: EDITOR_WIDTH,
            height: EDITOR_HEIGHT,
        },
    };

    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect,
            style,
            objc2_app_kit::NSBackingStoreType::Buffered,
            false,
        )
    };
    window.setTitle(&objc2_foundation::NSString::from_str("Overbridge Host"));
    if visible {
        window.makeKeyAndOrderFront(None);
    } else {
        window.orderBack(None);
    }

    let parent = window.contentView().context("NSWindow contentView")?;
    let parent_ptr = parent.as_ref() as *const NSView as *mut c_void;

    let mut guard = plugin.lock();
    match guard.editor() {
        Some(editor) => {
            editor
                .open(WindowHandle::NSView(parent_ptr), 1.0)
                .context("open Overbridge editor")?;
            editor.show();
            if let Some((w, h)) = editor.size() {
                editor.set_size(w, h);
                let frame = NSRect {
                    origin: NSPoint { x, y },
                    size: NSSize {
                        width: f64::from(w),
                        height: f64::from(h),
                    },
                };
                window.setFrame_display(frame, true);
            }
            tracing::info!(
                "Overbridge plugin editor open ({})",
                if visible { "visible" } else { "hidden" }
            );
        }
        None => {
            EDITOR_OPEN.store(false, Ordering::SeqCst);
            tracing::warn!("Plugin has no editor — hardware sync may not work");
        }
    }

    std::mem::forget(window);

    pump_main_run_loop_once(2.0);
    if let Some(mut guard) = plugin.try_lock() {
        if let Some(editor) = guard.editor() {
            editor.on_idle();
        }
    }
    Ok(())
}

fn pump_main_run_loop_once(seconds: f64) {
    let run_loop = NSRunLoop::mainRunLoop();
    let date = NSDate::dateWithTimeIntervalSinceNow(seconds);
    run_loop.runUntilDate(&date);
}

#[cfg(not(target_os = "macos"))]
mod stub {
    use super::*;
    use crossbeam_channel::Receiver;
    use std::sync::Arc;
    use parking_lot::Mutex;
    use truce_rack::vst3::Vst3Plugin;

    pub fn init_appkit() {}

    pub struct EditorPump;

    impl EditorPump {
        pub fn start(
            _plugin: Arc<Mutex<Vst3Plugin>>,
            _parameters: Arc<RwLock<Vec<ParameterSnapshot>>>,
            _open_on_start: bool,
            _visible_on_start: bool,
            _editor_requests: Receiver<()>,
            _param_flush: Receiver<()>,
            _param_refresh: Receiver<()>,
        ) -> Result<Self> {
            Ok(Self)
        }
        pub fn tick(&self) {}
        pub fn shutdown(self) {}
    }
}

#[cfg(not(target_os = "macos"))]
pub use stub::{EditorPump, init_appkit};
