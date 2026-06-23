# Design: Overbridge parameter & preset sync

How host ↔ device state stays in sync, what Overbridge does and does **not** tell us,
and why the current approach looks the way it does. This supersedes the strategy notes in
`../hardware-sync.md`.

## What we can access

`ob-host` only talks to the device through the **VST3 plugin**. The plugin owns the
proprietary Overbridge IPC (TCP on `127.0.0.1:46000` for messages, `:46010`/`:46011` for
async/audio, plus shared memory). We do **not** implement that protocol.

| Path | Direction | Mechanism |
|------|-----------|-----------|
| Host → device | UI/MIDI → hardware | `set_parameter` (on the caller thread, under the plugin lock) → `IParameterChanges` delivered by a host-driven `process()` |
| Device → host (knobs) | hardware → UI | plugin calls `IComponentHandler::performEdit` |
| Device → host (presets) | hardware preset → UI | **no callback** — see below |

> Parameter writes apply on the calling HTTP/WS thread rather than the audio command channel;
> see [`audio-and-control-api.md`](audio-and-control-api.md) for that decision.

## Empirical findings (verified live, via `vst_handler` logging)

On an Analog Heat MKII, changing a **preset on the device**:

- `IComponent::getState` blob **does** change.
- **No** VST3 callback fires — no `performEdit`, no `restartComponent`, no
  `notifyProgramListChange`, and `IHostApplication::createInstance` is **never** called.
  (So Overbridge is not even attempting the connection-point/`IMessage` path.)
- `IEditController::getParamNormalized` stays **stale** until the controller is nudged.

Individual **knob** moves *do* arrive via `performEdit`. Only **presets/settings** are
silent.

Conclusion: a purely event-driven design is impossible for presets — there is no event.
Detection must be polled; the sync itself must be triggered explicitly.

## The mechanism that works: `getState` → `setComponentState`

The fix is the documented host-on-load sequence:

1. **Detect**: on the main run loop, hash `IComponent::getState` (`plugin_state_fingerprint`)
   every ~100 ms. A changed hash means preset/settings (or our own edit — see gating).
2. **Refresh the controller**: `component.getState()` → `controller.setComponentState(bytes)`
   (`push_component_state_to_controller`). This makes the controller re-read parameter
   values from the processor state.
   - Quirk: Overbridge's `setComponentState` **returns `3` (`kNotImplemented`)** but the
     refresh *does* take effect, and it then emits `restartComponent(kParamValuesChanged)`.
     Treat the return code as informational, not fatal.
3. **Read back**: force a `getParamNormalized` scan, diff against the cache, push changes to
   the WebSocket.

## What NOT to do (previous bug)

The earlier implementation reacted to a state change by shipping the blob to the **audio
thread** and doing `setProcessing(0) → component.setState(its own bytes) → setProcessing(1)`
plus a controller state round-trip. This **clobbered the live values** Overbridge had just
pushed in and never called `setComponentState`, so `getParamNormalized` stayed stale and the
UI never updated. The "fix" was the bug. All of that machinery
(`ReapplyPluginState`, `reapply_component_state`, `refresh_after_hardware_state`) was removed.

## Distinguishing our own edits from device presets

The fingerprint also trips on the host's own writes, which must **not** be treated as preset
loads (doing so re-applies state mid-edit and arms an expensive refresh). We gate detection
on recent edit activity in either direction:

- `hardware_edit_active(800ms)` — set when the plugin calls `performEdit` (hardware knob).
- `host_edit_active(800ms)` — set in `Vst3Plugin::set_parameter` (UI / MIDI / macro).

If either is active, a fingerprint change is assumed to be the user's edit and detection is
skipped.

## Cost control (still being tuned — see active issue)

A full scan is 2115 params under the plugin lock, contending with the 48 kHz audio callback.
Current throttles:

- Routine catch-all scan at ~10 Hz (`SYNC_INTERVAL_TICKS`).
- Post-preset burst (`STATE_REFRESH_BURST_TICKS`, ~2 s, since Overbridge applies presets via
  `UpdateSettingsAsync` asynchronously) scans every 8th tick (`BURST_SYNC_STRIDE`).
- No param scanning on the audio thread.

Residual jitter remains; see `../active-issues/jitter-on-param-sync.md`.

## If we ever wanted true event-driven sync

Would require either Overbridge to emit `restartComponent`/messages on preset load (it does
not), or reverse-engineering the Overbridge IPC on `:46000` (`ElektronIpcMessages`,
`RemoteIpcMessageConnection`) — undocumented, proprietary, out of scope.

## Related code

- `src/host/param_sync.rs` — fingerprint + `sync_params_from_plugin`.
- `src/host/editor_macos.rs` — detection, gating, scan throttle.
- `vendor/truce-rack-vst3/src/lib.rs` — `push_component_state_to_controller`, `set_parameter`.
- `vendor/truce-rack-vst3/src/host_services.rs` — edit-activity tracking, handler logging.
