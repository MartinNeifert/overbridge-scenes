//! Optional baseview window for embedding the Overbridge plugin editor.
//! Must run on the main thread (baseview requirement on macOS).

use std::sync::Arc;

use anyhow::{Context, Result};
use baseview::{Event, EventStatus, Size, Window, WindowHandler, WindowOpenOptions, WindowScalePolicy};
use dispatch::Queue;
use parking_lot::Mutex;
use raw_window_handle::{HasRawWindowHandle, RawWindowHandle};
use truce_rack::vst3::Vst3Plugin;
use truce_rack_core::editor::WindowHandle as PluginParent;
use truce_rack_core::plugin::PluginCore;

use crate::host::plugin_backend::SharedPlugin;

const INITIAL_WINDOW: (u32, u32) = (320, 240);

pub struct GuiWindow {
    _started: (),
}

impl GuiWindow {
    pub fn start(plugin: SharedPlugin, title: String) -> Result<Self> {
        Queue::main().exec_async(move || {
            if let Err(e) = run_window(plugin, title) {
                tracing::error!("GUI window error: {e:#}");
            }
        });
        Ok(Self { _started: () })
    }

    pub fn shutdown(self) {}
}

fn run_window(plugin: SharedPlugin, title: String) -> Result<()> {
    let plugin_for_handler = Arc::clone(&plugin);
    let window_opts = WindowOpenOptions {
        title,
        size: Size::new(
            f64::from(INITIAL_WINDOW.0),
            f64::from(INITIAL_WINDOW.1),
        ),
        scale: WindowScalePolicy::SystemScaleFactor,
    };

    Window::open_blocking(window_opts, move |window| {
        let parent = raw_handle_to_plugin_handle(window.raw_window_handle());
        let mut editor_size = None;
        {
            let mut guard = plugin.lock();
            let vst3 = guard.vst3_mut().context("GUI requires VST3 plugin")?;
            if let Some(editor) = vst3.editor() {
                if let Err(e) = editor.open(parent, 1.0) {
                    tracing::error!("editor.open failed: {e}");
                } else {
                    editor.show();
                    editor_size = editor.size();
                    tracing::info!("Overbridge editor open in baseview window");
                }
            } else {
                tracing::warn!("Plugin has no editor");
            }
        }
        if let Some((w, h)) = editor_size {
            window.resize(Size::new(f64::from(w), f64::from(h)));
            let mut guard = plugin.lock();
            if let Ok(vst3) = guard.vst3_mut() {
                if let Some(editor) = vst3.editor() {
                    editor.set_size(w, h);
                }
            }
        }

        GuiHandler {
            plugin: Arc::clone(&plugin_for_handler),
        }
    });
    Ok(())
}

struct GuiHandler {
    plugin: SharedPlugin,
}

impl WindowHandler for GuiHandler {
    fn on_frame(&mut self, _window: &mut Window) {
        if let Some(mut guard) = self.plugin.try_lock() {
            if let Ok(vst3) = guard.vst3_mut() {
                if let Some(editor) = vst3.editor() {
                    editor.on_idle();
                }
            }
        }
    }

    fn on_event(&mut self, _window: &mut Window, _event: Event) -> EventStatus {
        EventStatus::Ignored
    }
}

fn raw_handle_to_plugin_handle(handle: RawWindowHandle) -> PluginParent {
    match handle {
        RawWindowHandle::AppKit(h) => PluginParent::NSView(h.ns_view),
        RawWindowHandle::Win32(h) => PluginParent::HWND(h.hwnd),
        #[allow(clippy::useless_conversion)]
        RawWindowHandle::Xlib(h) => PluginParent::X11(h.window.into()),
        other => panic!("unsupported raw-window-handle variant: {other:?}"),
    }
}
