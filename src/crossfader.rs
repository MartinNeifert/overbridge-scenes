//! Server-side crossfader morph apply — used by the remote VST and HTTP API.

use anyhow::{Context, Result};
use ob_morph::{self, BaselineState, MorphContext, ScenesDocument};

use crate::state::AppState;

#[derive(Debug, serde::Deserialize)]
pub struct CrossfaderApplyRequest {
    /// Pattern key (`A01`…`P16`). Uses the active pattern from the scenes UI when omitted.
    #[serde(default)]
    pub pattern: Option<String>,
    /// A/B crossfader position (0..1). Uses stored position when omitted.
    #[serde(default)]
    pub pos: Option<f64>,
    /// Quad pad X (0..1).
    #[serde(default)]
    pub x: Option<f64>,
    /// Quad pad Y (0..1).
    #[serde(default)]
    pub y: Option<f64>,
}

#[derive(Debug, serde::Serialize)]
pub struct CrossfaderApplyResponse {
    pub pattern: String,
    pub applied: usize,
}

pub fn apply_crossfader(state: &AppState, req: &CrossfaderApplyRequest) -> Result<CrossfaderApplyResponse> {
    let plugin = state.plugin_info().name;
    let pattern = resolve_pattern(state, &plugin, req.pattern.as_deref())?;

    let scenes_json = state
        .scenes_store()
        .load(&plugin, &pattern)?
        .unwrap_or_else(|| serde_json::json!({
            "scenes": [],
            "crossfader": {"a": null, "b": null, "pos": 0},
            "baseline": {"explicit": false, "values": []}
        }));

    let doc: ScenesDocument = serde_json::from_value(scenes_json).context("parse scenes document")?;
    let stored_cf = doc.crossfader.clone().unwrap_or_default();
    let crossfader = ob_morph::crossfader_with_position(&stored_cf, req.pos, req.x, req.y);

    let baseline = doc.baseline.clone().unwrap_or(BaselineState {
        explicit: false,
        values: Vec::new(),
    });

    let live_params: Vec<(usize, f64, f64, f64)> = {
        let host = state.host();
        host.parameters()
            .iter()
            .map(|p| (p.index, p.min, p.max, p.value))
            .collect()
    };

    let ctx = MorphContext::from_params(&baseline, &live_params);
    let updates = ob_morph::compute_crossfade_updates(&crossfader, &doc.scenes, &ctx);

    if updates.is_empty() {
        return Ok(CrossfaderApplyResponse {
            pattern,
            applied: 0,
        });
    }

    let batch: Vec<(usize, f64)> = updates.iter().map(|u| (u.index, u.value)).collect();
    state
        .host()
        .set_parameters_batch(&batch)
        .context("apply crossfader batch")?;

    Ok(CrossfaderApplyResponse {
        pattern,
        applied: updates.len(),
    })
}

fn resolve_pattern(state: &AppState, plugin: &str, requested: Option<&str>) -> Result<String> {
    if let Some(p) = requested {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_uppercase());
        }
    }
    if let Some(active) = state.scenes_store().load_active_pattern(plugin)? {
        return Ok(active);
    }
    Ok("A01".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::fake_app_state_with_scenes_dir;
    use std::path::PathBuf;

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!("ob-xf-apply-{}", std::process::id()))
    }

    #[test]
    fn apply_crossfader_morphs_assigned_scenes() {
        let dir = temp_dir();
        let _ = std::fs::remove_dir_all(&dir);
        let state = fake_app_state_with_scenes_dir(dir.clone()).expect("app state");

        let payload = serde_json::json!({
            "scenes": [
                {"id":"1","name":"A","params":[{"index":0,"id":1,"name":"Filter Cutoff","value":0.0}]},
                {"id":"2","name":"B","params":[{"index":0,"id":1,"name":"Filter Cutoff","value":1.0}]}
            ],
            "crossfader": {"a":"1","b":"2","pos":0.0},
            "baseline": {"explicit":false,"values":[]}
        });
        state
            .scenes_store()
            .save("OB Test Host", "A01", &payload)
            .unwrap();

        let resp = apply_crossfader(
            &state,
            &CrossfaderApplyRequest {
                pattern: Some("A01".into()),
                pos: Some(0.75),
                x: None,
                y: None,
            },
        )
        .expect("apply");

        assert_eq!(resp.applied, 1);
        let cutoff = state.host().get_parameter(0).expect("cutoff");
        assert!((cutoff.value - 0.75).abs() < 1e-6);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
