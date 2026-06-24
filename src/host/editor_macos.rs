//! macOS: open the Overbridge editor on the main thread and pump
//! `NSRunLoop` so JUCE timers in `RemoteDeviceClient` can run.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, TryRecvError};
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSView, NSWindow, NSWindowStyleMask};
use objc2_foundation::{NSDate, NSPoint, NSRect, NSRunLoop, NSSize};
use parking_lot::Mutex;
use truce_rack_core::editor::WindowHandle;
use truce_rack_core::plugin::PluginCore;

use crate::host::param_sync_pump::ParamSyncPump;
use crate::host::plugin_backend::SharedPlugin;

const EDITOR_WIDTH: f64 = 900.0;
const EDITOR_HEIGHT: f64 = 700.0;
const PUMP_INTERVAL: f64 = 0.001;

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
    plugin: SharedPlugin,
    sync_pump: Mutex<ParamSyncPump>,
    shutdown: Receiver<()>,
    editor_requests: Receiver<()>,
    open_on_start: bool,
    visible_on_start: bool,
    opened_on_start: AtomicBool,
    started: Instant,
}

impl EditorPump {
    /// Prepare run-loop pumping. Call [`Self::tick`] from the main thread event loop.
    pub fn start(
        plugin: SharedPlugin,
        sync_pump: ParamSyncPump,
        open_on_start: bool,
        visible_on_start: bool,
        editor_requests: Receiver<()>,
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
                sync_pump: Mutex::new(sync_pump),
                shutdown: shutdown_rx,
                editor_requests,
                open_on_start,
                visible_on_start,
                opened_on_start: AtomicBool::new(!open_on_start),
                started: Instant::now(),
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

        pump_main_run_loop_once(PUMP_INTERVAL);

        let plugin = Arc::clone(&state.plugin);
        state.sync_pump.lock().tick(move |shared| {
            if let Some(mut guard) = shared.try_lock() {
                if let Ok(vst3) = guard.vst3_mut() {
                    if let Some(editor) = vst3.editor() {
                        editor.on_idle();
                    }
                }
            }
            let _ = plugin;
        });
    }

    pub fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
    }
}

fn open_editor_inner(plugin: &SharedPlugin, visible: bool) -> Result<()> {
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
    let vst3 = guard.vst3_mut().context("editor requires VST3 plugin")?;
    match vst3.editor() {
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
        if let Ok(vst3) = guard.vst3_mut() {
            if let Some(editor) = vst3.editor() {
                editor.on_idle();
            }
        }
    }
    Ok(())
}

fn pump_main_run_loop_once(seconds: f64) {
    let run_loop = NSRunLoop::mainRunLoop();
    let date = NSDate::dateWithTimeIntervalSinceNow(seconds);
    run_loop.runUntilDate(&date);
}
