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

## Why a standalone UI, no backend changes

The existing host already exposes everything needed:

- **Live values** — the WebSocket `parameters` / `param_updates` messages (and
  `GET /api/parameters` as a fallback) stream every parameter's current value,
  including hardware knob moves (see `overbridge-param-sync.md`).
- **Writes** — the WebSocket accepts `{action:"set_parameter", index, value}`,
  routed to the audio thread and applied to the VST3 parameter, then mirrored to
  the device by Overbridge.

So Scenes is implemented purely as a new browser surface — `web/scenes.html`,
`web/scenes.js`, `web/scenes.css` — served by the same static handler at
`/scenes.html`. The classic surface (`/`, `web/app.js`, `web/style.css`) is
untouched. This also keeps it working regardless of host platform, since no Rust
code (which is macOS-only for the audio/VST layer) had to change.

A scene write goes through the *same* command path as a manual edit, so the
classic surface, MIDI mappings, and the hardware all see the morph live.

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
  survive index shifts and stale params are dropped.

Persistence is `localStorage`, keyed `ob-scenes:v1:<plugin>:<pattern>`, so each
plugin (Digitakt vs Analog Heat) **and each pattern** keeps its own scenes and
A/B assignment. See [Per-pattern scenes](#per-pattern-scenes) below.

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

### Baseline

`baseline[i]` is a snapshot of the live device values for the union parameters,
captured when:

- an A/B assignment changes,
- the user clicks **Capture baseline**, or
- the crossfader is first touched while unarmed (e.g. after a page reload
  restored assignments).

It is deliberately *not* re-captured while morphing, so dragging the crossfader
back and forth is stable and reversible.

## Authoring flow ("Map")

1. Choose the scene to edit (**Add parameters to**).
2. Search → **＋** adds a parameter, capturing its current live value.
3. Turn the hardware knob → **⤓ Map** on the row re-snapshots the live value.
   **Snapshot live** does this for every parameter in the scene at once.
4. Per-row slider fine-tunes the stored value; **✕** removes it.
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
value hasn't changed beyond a small epsilon. They are sent over the WebSocket
(lowest latency); if it's down, the code falls back to `POST /api/parameters/{i}`.

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
