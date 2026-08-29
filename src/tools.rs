//! The MCP tools: rubric (calibration), baseline images (anchors), score
//! (verdict + follow-up cards), and history.

use serde_json::{Value, json};

use crate::gauge::{self, BASELINE_IMAGES_COLLECTION};
use crate::host::{HostFn, call_host};

fn require_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("'{key}' is required"))
}

/// `ui_gauge_rubric {}` — categories, bars, and the user's baseline rankings
/// (scores + notes, images by reference) so the calling agent scores on the
/// USER's calibrated 1-10 scale, not its own.
pub fn rubric_tool(_args: Value) -> Result<Value, String> {
    let cats = gauge::categories();
    let baselines = gauge::baselines();
    let categories: Vec<Value> = cats
        .iter()
        .map(|c| {
            json!({
                "key": c.key,
                "label": c.label,
                "bar": gauge::bar_for(c, &baselines),
            })
        })
        .collect();
    let anchors: Vec<Value> = baselines
        .iter()
        .map(|b| {
            json!({
                "id": b.id,
                "name": b.name,
                "notes": b.notes,
                "user_scores": b.scores,
            })
        })
        .collect();
    Ok(json!({
        "categories": categories,
        "baselines": anchors,
        "instructions": "Score the target UI 1-10 per category ON THE USER'S SCALE: each \
    baseline above was ranked by the user — fetch one or two with ui_gauge_baseline_image, \
    compare the target against them, and calibrate your numbers so a screenshot the user \
    would rank 6 gets a 6. Then call ui_gauge_score with every category scored. A category \
    at or above its bar passes; below the bar is subpar and creates follow-up work.",
    }))
}

/// `ui_gauge_baseline_image { id }` — one baseline's image, for calibration.
pub fn baseline_image_tool(args: Value) -> Result<Value, String> {
    let id = require_str(&args, "id")?;
    let img = gauge::store_get(BASELINE_IMAGES_COLLECTION, &id)?
        .ok_or_else(|| format!("no baseline image '{id}'"))?;
    Ok(img)
}

/// `ui_gauge_score { target, scores, notes? }` — verdict against the bars;
/// subpar auto-creates one card per gap when the caller has project scope.
pub fn score_tool(args: Value) -> Result<Value, String> {
    let target = require_str(&args, "target")?;
    let notes = args
        .get("notes")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let raw_scores = args
        .get("scores")
        .and_then(|v| v.as_object())
        .ok_or("'scores' is required: {category_key: 1-10, ...}")?;
    let mut scores = std::collections::BTreeMap::new();
    for (k, v) in raw_scores {
        let n = v
            .as_u64()
            .filter(|n| (1..=10).contains(n))
            .ok_or_else(|| format!("score for '{k}' must be an integer 1-10"))?;
        scores.insert(k.clone(), n as u8);
    }

    let cats = gauge::categories();
    let baselines = gauge::baselines();
    let gaps = gauge::judge(&scores, &cats, &baselines)?;
    let verdict = if gaps.is_empty() { "pass" } else { "subpar" };

    // Subpar → follow-up cards in the caller's project (best-effort: a chat
    // session without project scope still gets the verdict + gap list).
    let mut cards_created: Vec<Value> = Vec::new();
    let mut cards_note = Value::Null;
    if !gaps.is_empty() {
        let scope = call_host(HostFn::CallerScope, &json!({}))?;
        match scope.get("project_id").and_then(|v| v.as_str()) {
            Some(project_id) if !project_id.is_empty() => {
                for g in &gaps {
                    let title = format!(
                        "UI: improve {} on {target} — scored {}, bar {}",
                        g.label.to_lowercase(),
                        g.score,
                        g.bar
                    );
                    let description = format!(
                        "ui-gauge scored \"{target}\" below the user's design bar.\n\n\
Category: {} ({})\nScore: {} / bar {}\n\nEvaluator notes: {}\n\n\
Raise this category to at least the bar, then re-run the ui-gauge evaluation \
(ui_gauge_rubric → score the updated UI → ui_gauge_score).",
                        g.label,
                        g.category,
                        g.score,
                        g.bar,
                        if notes.is_empty() { "(none)" } else { &notes }
                    );
                    match call_host(
                        HostFn::CreateCard,
                        &json!({ "project_id": project_id, "title": title, "description": description }),
                    ) {
                        Ok(v) => {
                            let id = v
                                .get("card")
                                .and_then(|c| c.get("id"))
                                .cloned()
                                .unwrap_or(Value::Null);
                            cards_created.push(json!({ "title": title, "card_id": id }));
                        }
                        Err(e) => {
                            cards_note = json!(format!("card creation failed: {e}"));
                            break;
                        }
                    }
                }
            }
            _ => {
                cards_note = json!(
                    "caller has no project scope — no cards created; create follow-up \
                     work yourself from the gap list"
                );
            }
        }
    }

    let ts = gauge::clock();
    let eval = json!({
        "id": gauge::new_id("eval"),
        "ts": ts,
        "target": target,
        "scores": scores,
        "verdict": verdict,
        "gaps": gaps,
        "notes": notes,
        "cards_created": cards_created,
    });
    gauge::record_evaluation(eval.clone())?;

    Ok(json!({
        "verdict": verdict,
        "gaps": eval["gaps"],
        "cards_created": cards_created,
        "cards_note": cards_note,
        "message": if gaps.is_empty() {
            "All categories at or above the user's bar.".to_string()
        } else {
            format!(
                "{} categor(ies) below the bar — drive the follow-up work, then re-evaluate.",
                eval["gaps"].as_array().map(|a| a.len()).unwrap_or(0)
            )
        },
    }))
}

/// `ui_gauge_history { target? }` — past evaluations, newest first.
pub fn history_tool(args: Value) -> Result<Value, String> {
    let target = args
        .get("target")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty());
    let mut evals = gauge::evaluations(target);
    evals.truncate(25);
    Ok(json!({ "evaluations": evals }))
}
