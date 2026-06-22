# Machine: Elektron Analog Heat MKII

Device-specific implementation notes and observed behavior. This is the device the host
has been developed and tested against.

## At a glance

| Property | Value (observed) |
|----------|------------------|
| Device class | Stereo analog stereo effect / distortion processor |
| Overbridge plugin | `Analog Heat` (VST3) |
| VST3 architecture | Separate edit controller (component ≠ controller) |
| Parameters exposed | **2115** |
| Audio | **4 ch duplex @ 48 kHz**, block size 128 |
| Speaker arrangement | quad (`0x33` = L\|R\|Ls\|Rs), falls back to stereo if rejected |
| `IComponent::getState` size | ~**683 bytes** |
| Engine ports (inside plugin) | `127.0.0.1:46000` (messages), `:46010`/`:46011` (async/audio) |

## Run

```bash
RUST_LOG=info cargo run --release -- --plugin "Analog Heat"
# control surface: http://127.0.0.1:7780/
```

Requires Overbridge Engine running and the device connected in **Overbridge USB mode**.

## Observed VST3 callback behavior

With `RUST_LOG=info` (handler traffic is logged under target `vst_handler`):

- At init: `IHostApplication::getName`, then `restartComponent(kLatencyChanged)`.
- **Hardware knob move** → `IComponentHandler::performEdit(id, value)` streams in real time.
  This is the smooth, per-event path (drained via `param_change_rx`).
- **Preset / settings change on the device** → **no** callback at all. The only signal is
  that the `IComponent::getState` hash changes. No `performEdit`, no
  `notifyProgramListChange`, no `restartComponent`, and `createInstance` is never called.
- Calling `IEditController::setComponentState(getState())` after detecting the change:
  - returns **`3` (`kNotImplemented`)**, **but the refresh takes effect anyway**, and
  - the plugin then emits `restartComponent(kParamValuesChanged)`.

This device is the concrete evidence behind the sync design in
`../designs/overbridge-param-sync.md` — presets are silent, so detection is polled and the
controller is refreshed explicitly.

## A real preset change, end to end (current build)

```
Plugin component state changed (preset/settings load) — pushing state to controller
restartComponent  flags=4  kParamValuesChanged
Plugin parameters refreshed (N changed)
```

…and the web UI updates. A typical preset change reports only a handful of changed
parameters (e.g. 3–7), since most of the 2115 are unrelated to the active preset.

## Known device-specific quirks

- **2115 parameters** makes any "scan all" pass expensive; this is the main driver of the
  lock-contention jitter in `../active-issues/jitter-on-param-sync.md`. A device with fewer
  params would likely not show the issue as strongly.
- **`setComponentState` returns `kNotImplemented`** — do not gate behavior on its return
  code for this plugin.
- **Preset application is asynchronous** (`UpdateSettingsAsync`): values can take up to ~2 s
  to fully settle after the `getState` hash first changes, which is why a post-detection
  refresh burst exists.
- The hidden editor must be open and the main run loop pumped, or `RemoteDeviceClient` device
  IPC (and therefore hardware sync) does not run.

## Not yet verified / TODO for this machine

- Long-term stability of the `performEdit` path under rapid two-handed knob movement.
- Whether `notifyProgramListChange` ever fires for any device operation (never seen so far).
- Behavior across firmware/Overbridge versions (only the currently-installed version tested).
