# Design: audio engine, control API, and other decisions

Decisions not specific to VST3 or Overbridge sync.

## Duplex audio via cpal on the Overbridge device

`audio.rs` opens the Overbridge device as a **duplex** stream with cpal: an input stream
captures device channels into a shared buffer, and the output callback feeds those into the
plugin's `process()` and writes the result back.

- **Why duplex:** Overbridge devices are effect/IO endpoints (e.g. Analog Heat is 4-in/4-out
  @ 48 kHz). The plugin expects real input audio to process; output-only would starve it.
- Input and output configs are negotiated to the device sample rate (48 kHz) and a fixed
  block size (`block_size`, default 128). The input callback resizes its scratch buffers if
  the driver hands a larger frame count.
- Bus layout is chosen from channel count (`bus_layout_for_channels`): mono/stereo/discrete;
  the VST3 layer maps that to a speaker arrangement (`0x33` quad for 4 ch), falling back to
  stereo if the plugin rejects the wide layout.

## Parameter writes: caller thread; MIDI/macros: command channel

Parameter writes (`set_parameter`, `set_parameters_batch`, by-name) apply **directly on the
calling HTTP/WS thread**: the handler takes the plugin lock, writes the value(s), then runs a
host-driven `process()` (`deliver_pending_via_process`) so the queued `IParameterChanges`
actually reach the device. MIDI and macros still go as `HostCommand`s over a crossbeam channel
and are applied on the audio thread inside `apply_command`.

- **Why writes moved off the command channel:** routing every fader frame through the audio
  thread coupled UI latency to the callback and produced audible artifacts under contention.
  Applying under the plugin lock + an explicit `process()` delivers changes deterministically
  without waiting for the next audio block. `control_only` mode skips the extra `process()`
  (no device audio path is open).
- **Lock ordering:** writers take `plugin` before `parameters`; this invariant is documented
  in `plugin_host.rs` to avoid deadlock with the sync passes.
- Readback is decoupled: a `RwLock<Vec<ParameterSnapshot>>` cache is updated by the sync
  passes and read by the HTTP/WS handlers, so API reads never touch the plugin mutex.

## Control API: Axum HTTP + WebSocket on :7780

- REST for discrete control (`GET/POST /api/parameters…`, `/api/midi/…`), WebSocket for
  low-latency bidirectional control and parameter snapshots.
- `POST /api/parameters/batch` applies many updates under one plugin lock + one `process()`,
  so a multi-parameter change (a crossfader morph frame) reaches the hardware as a unit. The
  scenes UI uses this; see [`scenes-crossfader.md`](scenes-crossfader.md).
- A `param_epoch` atomic is bumped on every change so the WS layer can push deltas
  (`take_pending_ws_updates`) rather than full snapshots.
- Static web control surface served from `web/`, with a `Cache-Control: no-cache` response
  header (`tower-http` `SetResponseHeaderLayer`) so a rebuilt `scenes.js`/`scenes.html` is
  always picked up on a normal reload instead of a stale cached copy.

## MIDI bridge + virtual port

`midir` creates a virtual input port ("Overbridge Host Control") and maps incoming CC to
named parameters via `config/mappings*.json`. Lets hardware controllers / Max / TouchOSC
drive parameters without going through HTTP.

## Main run loop driven by tokio interval

`main.rs` runs a 4 ms tokio interval that calls `host.runloop_tick()` on the main thread,
which (a) drains `performEdit` values from `param_change_rx` into the cache and (b) pumps the
editor (`EditorPump::tick`).

- **Why the main thread:** AppKit / `NSRunLoop` and the plugin editor must be serviced on the
  process main thread; tokio's `current_thread` flavor keeps us there.

## Build / target-dir gotcha

If `CARGO_TARGET_DIR` is set (some sandboxed shells point it at a temp cache), `cargo build`
writes there, not `./target`, and running `./target/release/ob-host` launches a stale binary.
Prefer `cargo run`, or resolve the dir via `cargo metadata`. See
[`../architecture.md`](../architecture.md) "Development notes".
