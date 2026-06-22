# Active issue: audio artifacts / degradation in duplex monitoring

Slight audio artifacts (intermittent clicks, dropouts, occasional graininess)
were audible on the Digitakt's analog Main Out while ob-host runs the native
CoreAudio duplex + monitoring path (`--duplex`, commit `46e0fe4`). The stream was
otherwise healthy: steady ~375 callbacks/s, zero `AudioUnitRender` errors. This
doc lists the causes and optimizations, roughly in priority order.

> **Status: resolved.** The root cause was confirmed and fixed — see
> **Resolution** below. The clicks are gone in testing. Items #1–#4 and #2
> shipped; #5 was intentionally skipped; #6–#9 remain optional future polish.

## Resolution

A `lock-skips` counter added to `DuplexStats` showed that during editor-thread
contention bursts (preset/state loads) ~12 % of audio blocks (≈90 of ~750 per
2 s) could not acquire the plugin lock. Previously each of those blocks was
emitted as **silence** — the audible clicks. The fix (commit recorded in
`git log`):

- **#1** the monitor copy + device-output write now run **unconditionally**,
  before/independent of `try_lock()`; `process()` runs only when the lock is
  free, but skipping it no longer drops audio. (Primary fix.)
- **#2** the editor pump's single long critical section was split into two short
  holds (editor idle / state-detection, then the heavy param scan), so the audio
  thread acquires the lock more often — `lock-skips` settle to 0 after startup.
- **#3** scratch buffers preallocated to 4096 frames; blocks never truncate
  (tracked by an `oversize` counter, observed 0).
- **#4** removed per-callback heap allocations (stack arrays instead of `vec!`).
- Output writer now also handles non-interleaved CoreAudio layouts, and an
  input-render failure emits silence rather than garbage.

`#5` (lock-free event ring) was skipped: in the duplex path `pending_events` is
produced and consumed on the same audio thread (uncontended), so it would add
cross-module churn for no benefit. `#6`–`#9` below remain optional.

The analysis that led here follows.

All locations are in `src/host/coreaudio_duplex.rs` (`render_cb`) unless noted.

## How monitoring works today (recap)

`render_cb` runs on the device's real-time render thread once per block (128
frames @ 48 kHz). It: zeroes the device output, pulls device input via
`AudioUnitRender`, `try_lock()`s the plugin and runs `process()`, then copies the
device input (Main L/R) back to the device output (the "monitor"). The monitored
copy is what you hear.

## Likely causes & optimizations (priority order)

### 1. Monitoring is gated behind the plugin lock — biggest suspect

```rust
let Some(mut p) = ctx.plugin.try_lock() else {
    return 0;            // ← device output stays SILENCE for this block
};
```

When the editor/param-sync thread holds the plugin mutex, `try_lock()` fails and
the callback returns early. Because the monitor copy happens *after* this point,
**that block is emitted as silence** → a click/dropout. The
[param-sync jitter issue](jitter-on-param-sync.md) shows the editor pump holds
that lock for non-trivial work, so these collisions happen periodically — exactly
the "intermittent artifact" symptom.

**Fix:** decouple monitoring from the plugin lock. The monitor copy
(device input → device output) needs no plugin state, so do it
**unconditionally**, before/independent of `try_lock()`. If the lock isn't
available, still emit monitored audio and just skip `process()` for that block.
Audio then never drops out due to lock contention.

### 2. Editor-thread lock hold time / frequency — the root of #1

The audio thread can only ever skip `process()` if something else holds the lock.
`src/host/editor_macos.rs` performs heavy work while holding the plugin mutex
(`on_idle`, state fingerprint, `save_state`, `push_component_state_to_controller`,
`sync_params_from_plugin`).

**Fix options (any/all):**
- Shorten the critical section: snapshot what's needed under the lock, then do
  fingerprinting/serialization/param diffing *outside* it.
- Lower the editor pump cadence (idle less often).
- Keep the parameter snapshot behind its own `RwLock` (already partly the case)
  so reads don't contend with the audio thread.

Combined with #1, this removes both the cause and the consequence of dropouts.

### 3. Variable / larger device buffer size → truncated blocks

```rust
let frames = (in_number_frames as usize).min(ctx.max_block);   // max_block = 128
...
let n = dst.len().min(need);    // need = out_ch * frames (clamped)
```

We *request* a 128-frame device buffer but the device may not honor it. If the
device ever delivers `in_number_frames > 128`, we process only 128 frames and the
**tail of the output buffer stays silent** (it was zeroed up top) → a periodic
gap/click locked to the buffer period.

**Fix:** size the scratch buffers to the actual `in_number_frames` (or make
`max_block` the true device buffer size) and stop clamping `frames`. Verify the
negotiated `kAudioDevicePropertyBufferFrameSize` matches the configured block,
and log a warning if they differ.

### 4. Per-callback heap allocation on the real-time thread

Every callback allocates:

```rust
let inputs: Vec<&[f32]> = vec![&ctx.dummy_in[..frames]];
let mut outputs: Vec<&mut [f32]> = vec![&mut ctx.dummy_out[..frames]];
let mut events = EventList::default();
let mut out_events = EventList::default();
```

Allocating (and freeing) on the audio thread can block on the allocator and cause
sporadic glitches. `EventList::default()` likewise may allocate.

**Fix:** preallocate these once in `CallbackCtx` and reuse them each callback
(clear, don't reallocate). Use fixed-capacity buffers / `SmallVec` for the bus
slice wrappers, and a reusable, capacity-reserved `EventList`.

### 5. A second lock on the audio thread (pending events)

```rust
if let Some(mut pend) = ctx.pending_events.try_lock() { ... }
```

A second `try_lock` on the hot path; if the producer side ever holds it during a
heavy push, events are silently deferred and it's one more contention point.

**Fix:** replace the `Mutex<Vec<Event>>` with a lock-free SPSC ring
(`crossbeam` / `rtrb`) drained on the audio thread, so the RT thread never takes a
mutex.

### 6. Sample rate is hard-coded to 48 kHz

`run_coreaudio_duplex` sets `sr = 48_000.0` and we set that ASBD on the unit. If
the device's nominal rate is ever 44.1 kHz (or changes), CoreAudio inserts an
implicit converter or the stream mismatches → pitch/quality artifacts.

**Fix:** query the device's nominal sample rate
(`kAudioDevicePropertyNominalSampleRate`) and activate the plugin + ASBDs at that
rate. Re-handle rate-change notifications.

### 7. Output AudioBufferList assumed interleaved / single buffer

The output write only fills `buffers.first_mut()` and assumes one interleaved
Float32 buffer:

```rust
if let Some(b) = buffers.first_mut() { ... copy_from_slice ... }
```

If CoreAudio hands us a **non-interleaved** layout (one buffer per channel), only
channel 0 is written and the rest stay silent → mono/missing-channel artifacts.

**Fix:** branch on `mNumberBuffers` / the negotiated ASBD: handle both
interleaved (one buffer) and non-interleaved (N buffers) output, de-interleaving
`output_scratch` as needed.

### 8. Gain staging / monitor level

Device-in peaks at ~0.10–0.14 (≈ −20…−17 dBFS) with `monitor_gain = 1.0`. If the
level is made up downstream, the noise floor and any quantization are amplified
too, which can read as "degradation."

**Fix:** confirm the cleanest source channels (`monitor_source`) and provide
sensible gain; consider a soft limiter only if clipping is observed. Avoid summing
gain that pushes transients over 0 dBFS.

### 9. Real-time thread scheduling (macOS Audio Workgroups)

The render proc is already a CoreAudio RT thread, but our extra work
(`AudioUnitRender` pull + copy) benefits from joining the device's
**AudioWorkgroup** so the scheduler treats it as part of the audio deadline group.

**Fix:** fetch `kAudioDevicePropertyIOThreadOSWorkgroup` and join it for the
duration of the callback (advanced; do after 1–4).

## Suggested order of work

1. **#1 + #2** — decouple monitor from the plugin lock and trim the editor
   critical section. Most likely to remove the audible artifacts outright.
2. **#3** — guarantee block-size alignment (no truncated tails).
3. **#4 + #5** — make the callback allocation-free and lock-free.
4. **#6 + #7** — robustness for sample rate and non-interleaved output.
5. **#8 + #9** — gain staging and workgroup scheduling polish.

## How to validate

- Re-enable a peak/under-run counter in `DuplexStats` (e.g. count blocks where
  `try_lock()` failed, and blocks where `in_number_frames > max_block`); log per
  2 s alongside the existing line. Artifacts should correlate with non-zero
  counts before the fix and zero after.
- A/B by temporarily forcing `process()` to be skipped every callback while
  keeping monitoring on: if audio stays clean, #1 is confirmed as the cause.

## Related

- [`jitter-on-param-sync.md`](jitter-on-param-sync.md) — the editor/audio lock
  contention that drives cause #1/#2.
- [`../designs/audio-cutout-and-duplex-fix.md`](../designs/audio-cutout-and-duplex-fix.md)
  — the duplex + monitoring design these optimizations refine (`46e0fe4`).
