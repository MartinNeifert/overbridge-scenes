# Active issue: jitter / choppiness during parameter changes

**Status:** open (implementation paused)
**Severity:** medium — functionality works, but motion is not consistently smooth
**Component:** `src/host/editor_macos.rs` (run-loop pump), `src/host/audio.rs` (cpal callback)

## Symptom

When parameters move — in **either** direction (hardware knob → web UI, or web UI →
hardware) — motion is mostly smooth but periodically hitches: roughly every ~2 s the
stream of updates goes choppy for ~0.5 s, then recovers. Audio can glitch in the same
windows.

This persists after the first round of mitigations below; the cadence is less regular
now but jitter is still observable.

## Architecture context

Three threads contend for one `parking_lot::Mutex<Vst3Plugin>`:

- **Audio thread** (cpal output callback) — locks the plugin every block
  (128 frames @ 48 kHz ≈ **2.67 ms**) to run `process()`.
- **Main thread** (AppKit run-loop pump, `EditorPump::tick`) — driven by the tokio
  interval at **4 ms**; pumps `NSRunLoop`, runs editor `on_idle`, polls the state
  fingerprint, and scans parameters.
- **API/WS threads** — read the parameter snapshot cache (separate `RwLock`, not the
  plugin mutex).

The plugin exposes **2115 parameters** (Analog Heat). Any "scan all parameters" pass is
2115 × (`getParameterInfo` + `getParamNormalized` [+ `getParamStringByValue`]) COM calls
while holding the plugin mutex. If that overlaps the audio callback's 2.67 ms deadline,
the callback is delayed → dropout, and UI updates arrive in bursts.

## Root causes identified so far

1. **The host's own edits were misread as device preset loads.** The fingerprint poll
   (`getState` hash) cannot distinguish "user is editing" from "device loaded a preset."
   UI→hardware edits emit no `performEdit`, so every slider drag looked like a preset and
   fired `setComponentState` + armed a 2 s full-refresh burst ~10×/second (observed: 49
   detections in one short editing session). `setComponentState` re-applying state mid-edit
   also produced visible value "jumps."
2. **250 Hz full-parameter sweep.** `sync_params_from_plugin` ran every 4 ms tick, sweeping
   all 2115 params under the plugin lock.
3. **Non-realtime-safe scan inside the audio callback.** `audio.rs` re-read all 2115 params
   every ~47 blocks (~125 ms) *on the audio thread itself*.

## Mitigations already applied (still jittery)

- Added `host_edit_active()` (timestamp set in `Vst3Plugin::set_parameter`) and gated
  preset detection on `hardware_edit_active || host_edit_active` (800 ms window).
- Throttled the routine scan to ~10 Hz (`SYNC_INTERVAL_TICKS = 25`) and the post-preset
  burst scan to every 8th tick (`BURST_SYNC_STRIDE`).
- Removed the per-callback param scan from the audio thread entirely.

## Leading remaining hypotheses (not yet verified)

- **Lock contention from the ~10 Hz full scan.** Even at 10 Hz, one pass touches 2115
  params under the plugin mutex; a single pass can exceed the 2.67 ms audio deadline.
  *Candidate fix:* chunk the scan across ticks (e.g. N params/tick), or drop the lock
  between sub-ranges; or only scan a "dirty"/pinned subset.
- **`NSRunLoop` pump cost.** `pump_main_run_loop_once(0.001)` plus up to 3× `editor.on_idle()`
  per tick happen under the plugin lock. JUCE timer work (`RemoteDeviceClient`) on a burst
  could spike. *Candidate:* measure `on_idle` duration; avoid holding the plugin lock across
  the run-loop pump.
- **`save_state()` cost on detection.** `IComponent::getState` serializes full plugin state
  (~683 bytes here, but the call may do more work internally) on the main thread under lock.
- **cpal buffer size / scheduling.** Block size 128 is tight; the main thread is not
  realtime-priority. The ~2 s cadence may correlate with an Overbridge engine housekeeping
  timer rather than our code — needs correlation against `vst_handler` logs and engine
  activity.

## How to reproduce / instrument

```bash
RUST_LOG=info cargo run --release -- --plugin "Analog Heat"
# Move a knob / drag the UI continuously; watch for repeated:
#   "pushing state to controller"   (should now be rare during edits)
#   "Plugin parameters refreshed (N changed)"
```

Useful next step: add timing spans around (a) the param scan, (b) the run-loop pump,
(c) `save_state`, and log when any exceeds ~2 ms, then correlate with audible glitches.

## Related code

- `src/host/editor_macos.rs` — `EditorPump::tick`, fingerprint poll, scan throttle.
- `src/host/audio.rs` — cpal duplex callback, plugin lock.
- `src/host/param_sync.rs` — `sync_params_from_plugin`, `plugin_state_fingerprint`.
- `vendor/truce-rack-vst3/src/host_services.rs` — `hardware_edit_active`, `host_edit_active`.
