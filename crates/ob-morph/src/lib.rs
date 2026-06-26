//! Crossfader morph math — Rust port of `web/scenes-morph.mjs`.
//!
//! Values are VST3-normalized (usually 0..1). Morphing is linear interpolation in
//! that space unless pickup/scale takeover is active during a fader grab.

use serde::Deserialize;
use std::collections::{HashMap, HashSet};

pub const EPS: f64 = 1e-4;

pub const DEFAULT_QUAD_CORNERS: QuadCorners = QuadCorners {
    tl: "1",
    tr: "2",
    bl: "3",
    br: "4",
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuadCorners {
    pub tl: &'static str,
    pub tr: &'static str,
    pub bl: &'static str,
    pub br: &'static str,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SceneParam {
    pub index: usize,
    #[serde(default)]
    pub id: Option<u32>,
    #[serde(default)]
    pub name: Option<String>,
    pub value: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Scene {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub params: Vec<SceneParam>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CrossfaderState {
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub a: Option<String>,
    #[serde(default)]
    pub b: Option<String>,
    #[serde(default)]
    pub pos: Option<f64>,
    #[serde(default)]
    pub corners: Option<HashMap<String, String>>,
    #[serde(default)]
    pub x: Option<f64>,
    #[serde(default)]
    pub y: Option<f64>,
    #[serde(default)]
    pub quad_center_mode: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BaselineEntry {
    pub index: usize,
    #[serde(default)]
    pub id: Option<u32>,
    pub value: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct BaselineState {
    #[serde(default)]
    pub explicit: bool,
    #[serde(default)]
    pub values: Vec<BaselineEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScenesDocument {
    #[serde(default)]
    pub scenes: Vec<Scene>,
    #[serde(default)]
    pub crossfader: Option<CrossfaderState>,
    #[serde(default)]
    pub baseline: Option<BaselineState>,
}

#[derive(Debug, Clone)]
pub struct ParamRange {
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone)]
pub struct MorphContext {
    pub baseline_explicit: bool,
    pub baseline: HashMap<usize, f64>,
    pub live_values: HashMap<usize, f64>,
    pub param_ranges: HashMap<usize, ParamRange>,
}

impl MorphContext {
    pub fn from_params(
        baseline: &BaselineState,
        live_params: &[(usize, f64, f64, f64)], // index, min, max, value
    ) -> Self {
        let mut baseline_map = HashMap::new();
        for entry in &baseline.values {
            baseline_map.insert(entry.index, entry.value);
        }
        let mut live_values = HashMap::new();
        let mut param_ranges = HashMap::new();
        for &(index, min, max, value) in live_params {
            live_values.insert(index, value);
            param_ranges.insert(index, ParamRange { min, max });
        }
        Self {
            baseline_explicit: baseline.explicit,
            baseline: baseline_map,
            live_values,
            param_ranges,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParamUpdate {
    pub index: usize,
    pub value: f64,
}

pub fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    v.min(hi).max(lo)
}

pub fn crossfader_mode(crossfader: &CrossfaderState) -> &str {
    if crossfader.mode.as_deref() == Some("quad") {
        "quad"
    } else {
        "ab"
    }
}

pub fn scene_by_id<'a>(scenes: &'a [Scene], id: &str) -> Option<&'a Scene> {
    scenes.iter().find(|s| s.id == id)
}

fn corners_map(crossfader: &CrossfaderState) -> HashMap<String, String> {
    let mut out = HashMap::new();
    out.insert("tl".to_string(), DEFAULT_QUAD_CORNERS.tl.to_string());
    out.insert("tr".to_string(), DEFAULT_QUAD_CORNERS.tr.to_string());
    out.insert("bl".to_string(), DEFAULT_QUAD_CORNERS.bl.to_string());
    out.insert("br".to_string(), DEFAULT_QUAD_CORNERS.br.to_string());
    if let Some(custom) = &crossfader.corners {
        for (k, v) in custom {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

pub fn union_indices(crossfader: &CrossfaderState, scenes: &[Scene]) -> Vec<usize> {
    let mut set = HashSet::new();
    if crossfader_mode(crossfader) == "quad" {
        let corners = corners_map(crossfader);
        for id in corners.values() {
            if let Some(scene) = scene_by_id(scenes, id) {
                for p in &scene.params {
                    set.insert(p.index);
                }
            }
        }
    } else {
        for id in [crossfader.a.as_deref(), crossfader.b.as_deref()] {
            if let Some(id) = id {
                if let Some(scene) = scene_by_id(scenes, id) {
                    for p in &scene.params {
                        set.insert(p.index);
                    }
                }
            }
        }
    }
    let mut indices: Vec<_> = set.into_iter().collect();
    indices.sort_unstable();
    indices
}

pub fn base_value(index: usize, ctx: &MorphContext) -> f64 {
    if ctx.baseline_explicit {
        if let Some(&v) = ctx.baseline.get(&index) {
            return v;
        }
    }
    if let Some(&v) = ctx.live_values.get(&index) {
        return v;
    }
    if let Some(&v) = ctx.baseline.get(&index) {
        return v;
    }
    0.0
}

pub fn empty_side_value(index: usize, ctx: &MorphContext) -> f64 {
    if ctx.baseline_explicit {
        if let Some(&v) = ctx.baseline.get(&index) {
            return v;
        }
    }
    if let Some(&v) = ctx.live_values.get(&index) {
        return v;
    }
    if let Some(&v) = ctx.baseline.get(&index) {
        return v;
    }
    0.0
}

pub fn endpoint_value(scene: Option<&Scene>, index: usize, ctx: &MorphContext) -> f64 {
    if let Some(scene) = scene {
        if let Some(p) = scene.params.iter().find(|p| p.index == index) {
            return p.value;
        }
    }
    empty_side_value(index, ctx)
}

fn param_range(index: usize, ctx: &MorphContext) -> (f64, f64) {
    if let Some(r) = ctx.param_ranges.get(&index) {
        let min = r.min;
        let mut max = r.max;
        if (max - min).abs() < f64::EPSILON {
            max = min + 1.0;
        }
        (min, max)
    } else {
        (0.0, 1.0)
    }
}

pub fn morph_param_value(
    index: usize,
    t: f64,
    scene_a: Option<&Scene>,
    scene_b: Option<&Scene>,
    ctx: &MorphContext,
) -> f64 {
    let av = endpoint_value(scene_a, index, ctx);
    let bv = endpoint_value(scene_b, index, ctx);
    let value = av + (bv - av) * t;
    let (min, max) = param_range(index, ctx);
    clamp(value, min.min(max), min.max(max))
}

pub fn bilinear_weights(x: f64, y: f64) -> (f64, f64, f64, f64) {
    let x1 = clamp(x, 0.0, 1.0);
    let y1 = clamp(y, 0.0, 1.0);
    (
        (1.0 - x1) * (1.0 - y1),
        x1 * (1.0 - y1),
        (1.0 - x1) * y1,
        x1 * y1,
    )
}

pub fn is_quad_center(x: f64, y: f64) -> bool {
    (x - 0.5).abs() < EPS && (y - 0.5).abs() < EPS
}

fn bilinear_weights_assigned(
    x: f64,
    y: f64,
    corner_assigned: &[bool; 4],
) -> (f64, f64, f64, f64) {
    let (mut tl, mut tr, mut bl, mut br) = bilinear_weights(x, y);
    let mut sum = 0.0;
    if corner_assigned[0] {
        sum += tl;
    } else {
        tl = 0.0;
    }
    if corner_assigned[1] {
        sum += tr;
    } else {
        tr = 0.0;
    }
    if corner_assigned[2] {
        sum += bl;
    } else {
        bl = 0.0;
    }
    if corner_assigned[3] {
        sum += br;
    } else {
        br = 0.0;
    }
    if sum <= EPS {
        return (tl, tr, bl, br);
    }
    (tl / sum, tr / sum, bl / sum, br / sum)
}

pub fn morph_quad_param_value(
    index: usize,
    x: f64,
    y: f64,
    corner_scenes: [Option<&Scene>; 4],
    ctx: &MorphContext,
    center_mode: &str,
) -> f64 {
    let (min, max) = param_range(index, ctx);
    let clamped = |v: f64| clamp(v, min.min(max), min.max(max));

    if center_mode == "baseline" && is_quad_center(x, y) {
        return clamped(base_value(index, ctx));
    }

    let corner_assigned = [
        corner_scenes[0].is_some(),
        corner_scenes[1].is_some(),
        corner_scenes[2].is_some(),
        corner_scenes[3].is_some(),
    ];

    let (w_tl, w_tr, w_bl, w_br) = if center_mode == "interpolation" {
        bilinear_weights_assigned(x, y, &corner_assigned)
    } else {
        bilinear_weights(x, y)
    };

    let weight_sum = w_tl + w_tr + w_bl + w_br;
    if weight_sum <= EPS {
        return clamped(base_value(index, ctx));
    }

    let mut value = 0.0;
    value += w_tl * endpoint_value(corner_scenes[0], index, ctx);
    value += w_tr * endpoint_value(corner_scenes[1], index, ctx);
    value += w_bl * endpoint_value(corner_scenes[2], index, ctx);
    value += w_br * endpoint_value(corner_scenes[3], index, ctx);
    clamped(value)
}

pub fn compute_crossfade_updates(
    crossfader: &CrossfaderState,
    scenes: &[Scene],
    ctx: &MorphContext,
) -> Vec<ParamUpdate> {
    if crossfader_mode(crossfader) == "quad" {
        return compute_quad_updates(crossfader, scenes, ctx);
    }

    let scene_a = crossfader.a.as_deref().and_then(|id| scene_by_id(scenes, id));
    let scene_b = crossfader.b.as_deref().and_then(|id| scene_by_id(scenes, id));
    if scene_a.is_none() && scene_b.is_none() {
        return Vec::new();
    }

    let t = crossfader.pos.unwrap_or(0.0);
    union_indices(crossfader, scenes)
        .into_iter()
        .map(|index| ParamUpdate {
            value: morph_param_value(index, t, scene_a, scene_b, ctx),
            index,
        })
        .collect()
}

pub fn compute_quad_updates(
    crossfader: &CrossfaderState,
    scenes: &[Scene],
    ctx: &MorphContext,
) -> Vec<ParamUpdate> {
    let corners = corners_map(crossfader);
    let corner_ids = [
        corners.get("tl").cloned(),
        corners.get("tr").cloned(),
        corners.get("bl").cloned(),
        corners.get("br").cloned(),
    ];
    let has_assigned = corner_ids
        .iter()
        .any(|id| id.as_ref().and_then(|id| scene_by_id(scenes, id)).is_some());
    if !has_assigned {
        return Vec::new();
    }

    let corner_scenes: [Option<&Scene>; 4] = [
        corner_ids[0]
            .as_deref()
            .and_then(|id| scene_by_id(scenes, id)),
        corner_ids[1]
            .as_deref()
            .and_then(|id| scene_by_id(scenes, id)),
        corner_ids[2]
            .as_deref()
            .and_then(|id| scene_by_id(scenes, id)),
        corner_ids[3]
            .as_deref()
            .and_then(|id| scene_by_id(scenes, id)),
    ];

    let x = crossfader.x.unwrap_or(0.5);
    let y = crossfader.y.unwrap_or(0.5);
    let center_mode = crossfader
        .quad_center_mode
        .as_deref()
        .unwrap_or("interpolation");

    union_indices(crossfader, scenes)
        .into_iter()
        .map(|index| ParamUpdate {
            value: morph_quad_param_value(index, x, y, corner_scenes, ctx, center_mode),
            index,
        })
        .collect()
}

/// Apply a runtime crossfader position override onto stored crossfader state.
pub fn crossfader_with_position(
    stored: &CrossfaderState,
    pos: Option<f64>,
    x: Option<f64>,
    y: Option<f64>,
) -> CrossfaderState {
    let mut cf = stored.clone();
    if let Some(p) = pos {
        cf.pos = Some(p);
    }
    if let Some(xv) = x {
        cf.x = Some(xv);
    }
    if let Some(yv) = y {
        cf.y = Some(yv);
    }
    cf
}

pub fn parse_scenes_document(json: &serde_json::Value) -> ScenesDocument {
    serde_json::from_value(json.clone()).unwrap_or(ScenesDocument {
        scenes: Vec::new(),
        crossfader: None,
        baseline: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_scenes() -> Vec<Scene> {
        vec![
            Scene {
                id: "1".into(),
                name: Some("Scene 1".into()),
                params: vec![SceneParam {
                    index: 0,
                    id: Some(1),
                    name: Some("Cutoff".into()),
                    value: 0.0,
                }],
            },
            Scene {
                id: "2".into(),
                name: Some("Scene 2".into()),
                params: vec![SceneParam {
                    index: 0,
                    id: Some(1),
                    name: Some("Cutoff".into()),
                    value: 1.0,
                }],
            },
        ]
    }

    fn test_ctx() -> MorphContext {
        MorphContext::from_params(
            &BaselineState {
                explicit: false,
                values: vec![BaselineEntry {
                    index: 0,
                    id: Some(1),
                    value: 0.0,
                }],
            },
            &[(0, 0.0, 1.0, 0.0)],
        )
    }

    #[test]
    fn ab_crossfade_midpoint() {
        let cf = CrossfaderState {
            a: Some("1".into()),
            b: Some("2".into()),
            pos: Some(0.5),
            ..Default::default()
        };
        let updates = compute_crossfade_updates(&cf, &test_scenes(), &test_ctx());
        assert_eq!(updates.len(), 1);
        assert!((updates[0].value - 0.5).abs() < 1e-6);
    }

    #[test]
    fn ab_crossfade_to_a() {
        let cf = CrossfaderState {
            a: Some("1".into()),
            b: Some("2".into()),
            pos: Some(0.0),
            ..Default::default()
        };
        let updates = compute_crossfade_updates(&cf, &test_scenes(), &test_ctx());
        assert!((updates[0].value - 0.0).abs() < 1e-6);
    }

    #[test]
    fn no_assigned_scenes_returns_empty() {
        let cf = CrossfaderState::default();
        let updates = compute_crossfade_updates(&cf, &test_scenes(), &test_ctx());
        assert!(updates.is_empty());
    }
}
