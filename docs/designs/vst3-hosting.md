# Design: VST3 hosting

How `ob-host` loads and drives Overbridge VST3 plugins, and why.

## No Steinberg SDK — community `vst3` bindings

We use the community `vst3` Rust crate (COM-style bindings) rather than the Steinberg
C++ SDK.

- **Why:** no git submodule, no CMake, no C++ toolchain. A fresh checkout builds in
  seconds. The plugin loading lifecycle (factory → component → controller) is small
  enough to implement directly in `vendor/truce-rack-vst3/src/lib.rs`.
- **Trade-off:** we implement only the host-side COM services Overbridge actually needs
  (`IHostApplication`, `IComponentHandler`/`2`, `IUnitHandler`/`2`, `IParameterChanges`,
  `IBStream`, `IEventList`). Anything a plugin expects that we haven't implemented either
  no-ops or returns `kNotImplemented`.

## macOS bundle loading via `CFBundle`, not raw `dlopen`

Plugins are loaded through `CFBundleCreate` + `CFBundleLoadExecutable`
(`lib.rs` `mac::MacBundle`), and `bundleEntry(CFBundleRef)` is called with the real
bundle ref.

- **Why:** JUCE/CoreFoundation plugins call into CFPlugin / AU registration during
  `bundleEntry`. Raw `dlopen` leaves the bundle unknown to CoreFoundation and the plugin
  dereferences garbage (crashes). Going through `CFBundle` gives the plugin the context it
  expects. This mirrors Steinberg's `module_mac.mm`.
- We deliberately **never** call `CFBundleUnloadExecutable` — plugins leave Obj-C class
  registrations and run-loop callbacks pointing into the dylib; unloading invalidates them.

## Separate-controller architecture, connection points wired directly

Overbridge uses the separate edit-controller model (component/processor and
`IEditController` are different COM objects). After creating both we connect their
`IConnectionPoint`s directly to each other (`a.connect(b); b.connect(a)`).

- **Why direct:** simplest wiring that lets the plugin's two halves talk.
- **Known limitation:** the VST3-recommended pattern is a host **ConnectionProxy** that
  marshals `IConnectionPoint::notify` messages onto the main/UI thread. We don't do this,
  and `IHostApplication::createInstance` returns `kNotImplemented`, so the plugin cannot
  allocate `IMessage`/`IAttributeList` to send across the connection. **Empirically this
  does not matter for Overbridge** — it never calls `createInstance` and never pushes
  messages on a preset change (verified via `vst_handler` logging). See
  `overbridge-param-sync.md`.

## Parameter delivery: `IParameterChanges` in `process()`, plus controller set

`Vst3Plugin::set_parameter` does three things:

1. Queues the change for delivery via `IParameterChanges` on the next `process()` call
   (`pending_param_changes`, coalesced to the latest value per ID).
2. Brackets it with `IEditControllerHostEditing::beginEditFromHost`/`endEditFromHost`.
3. Calls `controller.setParamNormalized` so the controller state matches immediately.

- **Why all three:** Overbridge expects sample-accurate parameter delivery through
  `IParameterChanges` (so the device hears it), but the controller's `getParamNormalized`
  must also reflect it for the host's readback/UI. Doing both keeps audio and UI consistent.

## Editor is opened (hidden) on the main thread

Even in headless/API use we open the plugin editor on a borderless off-screen `NSWindow`
and pump `NSRunLoop` (`editor_macos.rs`).

- **Why:** Overbridge's `RemoteDeviceClient` uses JUCE timers that only run on a live main
  run loop, and some device IPC is gated on the editor/view existing. Without the editor +
  pump, hardware sync does not work. `OB_NO_EDITOR=1` disables it; `OB_OPEN_EDITOR=1` makes
  it visible.

## Threading model

- One audio thread (cpal) owns `process()` under `Mutex<Vst3Plugin>`.
- One main thread pumps the run loop and polls/scuns parameters (also under the mutex,
  via `try_lock` so it never blocks the audio thread).
- API/MIDI threads send `HostCommand`s over a crossbeam channel to the audio thread, and
  read a separate `RwLock<Vec<ParameterSnapshot>>` cache for responses.
- **Why a single plugin mutex:** VST3 plugins are not thread-safe; all COM calls funnel
  through one lock. The cost of this choice is the contention described in
  `../active-issues/jitter-on-param-sync.md`.
