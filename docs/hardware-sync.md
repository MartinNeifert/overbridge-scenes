# Hardware sync and Overbridge IPC (overview)

How `ob-host` talks to Elektron hardware and what it can/cannot access.

> **Authoritative design:** see [`designs/overbridge-param-sync.md`](designs/overbridge-param-sync.md)
> for the current preset/parameter sync mechanism and the reasoning behind it, and
> [`machines/analog-heat-mk2.md`](machines/analog-heat-mk2.md) for device-specific behavior.
> This page is a high-level map only.

## Architecture

```
Web UI / HTTP / WS / MIDI
        │
        ▼
   ob-host (Rust)
        │  VST3 API only
        ▼
 Overbridge VST3 plugin (e.g. Analog Heat)
   RemoteDeviceClient · MessageDispatcher · UpdatePresetAsync …
        │  proprietary TCP + shared memory
        ▼
 Overbridge Engine (:46000, :46010, :46011)
        │  USB
        ▼
   Elektron hardware
```

`ob-host` does **not** implement Overbridge's IPC protocol. It loads the VST3 plugin and
lets the plugin own device communication.

## What we have access to

| Layer | Access | Notes |
|-------|--------|-------|
| **VST3 host API** | Yes | Parameters, `IComponent::getState`/`setState`, `IEditController`, component-handler callbacks |
| **CoreAudio (device listing)** | Yes | `system_profiler` metadata for the device selector UI; ob-host does not open the device for I/O |
| **AppKit run loop** | Yes | Hidden editor + run-loop pump so JUCE timers in `RemoteDeviceClient` run |
| **Overbridge TCP IPC** | No | Plugin connects to Engine on localhost; protocol is proprietary |
| **Shared memory** | No | Plugin ↔ Engine audio; not exposed to us |
| **`IHostApplication::createInstance`** | Stub | Returns `kNotImplemented`; Overbridge never calls it in practice |

## Key facts (see authoritative docs for detail)

- **Hardware knobs → UI** arrive via `IComponentHandler::performEdit` (real-time, per event).
- **Hardware presets → UI** emit **no** VST3 callback; only the `getState` blob changes. We
  detect that by polling a state fingerprint, then refresh the controller with
  `component.getState()` → `controller.setComponentState(...)`.
- **UI/MIDI → hardware** goes out via the edit controller (`setParamNormalized`) and the
  editor run-loop pump.

## Logging

```bash
# All handler traffic
RUST_LOG=info cargo run --release -- --plugin "Analog Heat"

# Handlers only
RUST_LOG=vst_handler=info,ob_host=warn cargo run --release -- --plugin "Analog Heat"
```

## Related host requirements

Overbridge plugins expect a real host context. `ob-host` provides `IHostApplication`,
`IComponentHandler`/`2`, `IUnitHandler`/`2`, sample-accurate `IParameterChanges`, and a
main-thread `NSRunLoop` pump + hidden editor. See
`vendor/truce-rack-vst3/src/host_services.rs`.
