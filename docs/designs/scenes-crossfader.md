# Design: Scenes & Crossfader (Octatrack-style)

Snapshot specific parameters into **4 scenes** and morph between two of them with
an Octatrack-style **A/B crossfader**. Target devices: Digitakt and Analog Heat,
but the feature is plugin-agnostic.

## Goal

- Given a loaded plugin (a Digitakt pattern, an Analog Heat preset, …), build a
  set of 4 scenes. Each scene is a snapshot of *chosen* parameters and the value
  each should take — exactly like an Octatrack scene, which only stores the
  parameter locks you set, not the whole state.
- A crossfader selects one scene (or none) per side and fades the live parameter
  values toward the configured values as the slider moves.
- Easy authoring: turn a knob on the hardware, click **Map**, and the current
  live value is saved as that scene's value for that parameter.

## Mostly a standalone UI

The feature is almost entirely a new browser surface — `web/scenes.html`,
`web/scenes.js`, `web/scenes.css` — served by the same static handler at
`/scenes.html`. The classic surface (`/`, `web/app.js`, `web/style.css`) is
untouched. It leans on what the host already exposes:

- **Live values** — the WebSocket `parameters` / `param_updates` messages (and
  `GET /api/parameters` as a fallback) stream every parameter's current value,
  including hardware knob moves (see `overbridge-param-sync.md`).
- **Writes** — applied via the control API (see below).

The one backend addition is `POST /api/parameters/batch`, which applies many
parameter updates under a single plugin lock and one `process()` pass. A morph
frame can touch dozens of parameters; batching avoids per-parameter lock churn
and delivers the whole frame to the device together. A scene write still goes
through the *same* apply path as a manual edit, so the classic surface, MIDI
mappings, and the hardware all see the morph live. See
[`audio-and-control-api.md`](audio-and-control-api.md).

## Data model

```
scene  = { id: "1".."4", name, params: [{ index, id, name, value }] }
xfader = { a: sceneId | null, b: sceneId | null, pos: 0..1 }
```

- There are always 4 fixed scene slots (a scene may be empty).
- `value` is the VST3-*normalized* parameter value (`param.min..param.max`,
  usually `0..1`). Morphing is linear interpolation in that space, which matches
  what `set_parameter` expects.
- Parameters are stored with `id` + `name` as well as `index`; on (re)connect or
  plugin switch the indices are re-resolved by `id` then `name`, so scenes
  survive index shifts and stale params are dropped (`validateScenes`).
- `validateScenes` runs on every full `parameters` broadcast (~2 s). It
  **mutates the existing param objects in place** rather than rebuilding them, so
  event-handler closures captured by the row sliders and **Map** buttons stay
  valid across re-syncs. (Rebuilding them was a real bug: slider edits landed on
  an orphaned object and silently did nothing.)

Persistence is `localStorage`, keyed `ob-scenes:v1:<plugin>:<pattern>`, so each
plugin (Digitakt vs Analog Heat) **and each pattern** keeps its own scenes, A/B
assignment, and captured baseline. See [Per-pattern scenes](#per-pattern-scenes)
below.

## Morph semantics

For crossfader position `t ∈ [0,1]`, over the union of parameters in A and B:

```
endpoint(scene, i) = scene-locked value if scene locks i, else baseline[i]
value(i)           = lerp(endpoint(A, i), endpoint(B, i), t)
```

- **Locked in both** scenes → morphs A-value ↔ B-value.
- **Locked in one** scene → morphs that lock ↔ baseline (the "unlocked" value),
  mirroring the Octatrack, where an unlocked side returns to the pattern value.
- **One side `None`** → morphs the other scene ↔ baseline. Assign a scene to B,
  leave A `None`, slide right: "fade current values → scene". Reverse for A.
- **Both `None`** → no-op.

### Baseline (per-pattern)

The baseline is the neutral "home" value used for the empty side of a morph — a
parameter only locked in one scene, or a side set to `— None —`. It is stored
**per pattern**, alongside that pattern's scenes, and has two flavours:

- **Explicit** — the user clicks **Capture baseline (pattern)**. This snapshots
  the live device values and marks the baseline `explicit`. The pattern now has a
  fixed home that morphs resolve against, persisted in `localStorage`.
- **Auto-seeded** — if a pattern has no explicit baseline yet, one is seeded
  silently (`captureBaseline({ silent: true, explicit: false })`) so morphing has
  something to interpolate toward. `ensureBaselineCoverage` extends it to any
  parameters that later join a scene.

`baseValue(i)` prefers the stored baseline **only when it is explicit and covers
`i`**; otherwise it falls back to the current live value. This is what stops a
stale auto-seeded `0` from dominating a freshly turned-up knob (the earlier
"slides back to 0 in jump mode" bug).

When a crossfader **grab** begins, the empty-side value for each parameter is
**frozen for the duration of that drag** (`emptySideValue`), so dragging back and
forth is stable and reversible and doesn't chase live updates mid-gesture. The
baseline itself is never re-captured while morphing.

## Authoring flow ("Map")

1. Choose the scene to edit (**Add parameters to**).
2. Search → **＋** adds a parameter, capturing its current live value.
3. Turn the hardware knob → **⤓ Map** on the row re-snapshots the live value.
   **Snapshot live** does this for every parameter in the scene at once.
4. Per-row slider fine-tunes the **scene's stored value** — the target the
   crossfader morphs toward. It edits scene state only and does **not** push to
   the device (moving the crossfader is what applies it). **✕** removes the row.
5. **Recall** applies a whole scene immediately, independent of the crossfader.

## Per-pattern scenes

On the Digitakt each **pattern** has its own sound, so scenes are namespaced per
pattern: every pattern keeps an independent set of 4 scenes (`localStorage` key
`ob-scenes:v1:<plugin>:<pattern>`). The active pattern is chosen in the **Pattern**
bar (bank `A–P` × number `1–16`); switching it saves the current pattern's scenes
and loads the target's. Pre-pattern scenes are migrated once into pattern `A01`.

### Can the active pattern be read from the VST? (investigation)

**No.** Empirically, the Digitakt Overbridge VST3 exposes **2711 parameters** —
all sound-engine, FX, per-track, and MIDI params. None is a pattern/program
selector:

- The only params matching "pattern" are per-track `T1..T8 Pattern Mute` toggles
  and `FX Master Pattern Volume` — not the pattern *index*.
- There is no `Program`, `Bank`, or `Pattern Select` parameter.
- Switching patterns on the device mutates the `IComponent::getState` blob (see
  `overbridge-param-sync.md`), but that fires **no callback** and only signals
  *"something changed"* — it cannot identify *which* pattern.

So the pattern index is not available through the VST3 parameter/state surface.

### How the active pattern is obtained

1. **Manual** — the Pattern bar (always works, no MIDI needed).
2. **MIDI Program Change follow** — the only live device signal of the pattern.
   Elektron devices send a Program Change (0–127) when the pattern changes (with
   *Program Change Send* enabled on the device). It is decoded as
   `bank = ⌊pc / 16⌋`, `number = pc mod 16` (PC 0 → A01, PC 16 → B01, …,
   PC 127 → H16) and auto-switches the active pattern. This reuses the page's
   Web MIDI access; point the **Follow Program Change** input at the device's
   MIDI port. Banks beyond H are reachable manually.

Both paths share one `setPattern()`, so manual and MIDI-driven switches behave
identically. No Rust/backend change was required.

## Performance

Crossfader drags can touch many parameters per frame. Writes are coalesced into a
per-`requestAnimationFrame` batch, de-duplicated per index, and dropped if the
value hasn't changed beyond a small epsilon. The frame is then sent as one
`POST /api/parameters/batch` request, so the whole morph step is applied under a
single plugin lock and one `process()` pass on the host.

## Limitations / future work

- Scenes live in the browser. A future enhancement could persist them host-side
  (per plugin) via a small `/api/scenes` endpoint so they're shared across
  machines and survive a cleared browser cache.
- The crossfader can be driven by a MIDI controller (absolute fader or endless
  encoder) via Web MIDI — see the **MIDI** row on the scenes page.
- Pattern follow relies on the device's *Program Change Send*; if that's off, use
  the manual Pattern bar. Banks beyond H aren't reachable by Program Change alone
  (would need Bank Select CC) but are always available manually.
- Morph is linear; per-parameter curves (log/exp) could be added like the MIDI
  mapper's `curve` field.
