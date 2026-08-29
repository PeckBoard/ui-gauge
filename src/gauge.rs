//! UI-gauge domain logic: categories, user-ranked baselines, the per-category
//! bar (median of the user's rankings unless overridden), verdicts, and the
//! evaluation history.
//!
//! The plugin is the rubric store, calibrator, verdict engine, and card
//! creator. It never looks at pixels — a vision-capable agent session does
//! the scoring, anchored to the user's own baseline rankings via
//! `ui_gauge_rubric` / `ui_gauge_baseline_image`.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::host::{HostFn, call_host};

pub const CATEGORIES_COLLECTION: &str = "categories";
pub const BASELINES_COLLECTION: &str = "baselines";
pub const BASELINE_IMAGES_COLLECTION: &str = "baseline_images";
pub const EVALUATIONS_COLLECTION: &str = "evaluations";
/// Generated baselines' full HTML documents, keyed by baseline id.
pub const BASELINE_HTML_COLLECTION: &str = "baseline_html";
pub const ENGINE_COLLECTION: &str = "engine";

pub const EVALUATIONS_CAP: usize = 100;
pub const DEFAULT_BAR: u8 = 7;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Category {
    pub key: String,
    pub label: String,
    /// User override; None = median of baseline rankings, else DEFAULT_BAR.
    #[serde(default)]
    pub bar_override: Option<u8>,
}

pub fn default_categories() -> Vec<Category> {
    [
        ("visual_hierarchy", "Visual hierarchy"),
        ("spacing_alignment", "Spacing & alignment"),
        ("typography", "Typography"),
        ("color_contrast", "Color & contrast"),
        ("consistency", "Consistency with the app"),
        ("accessibility", "Accessibility"),
    ]
    .into_iter()
    .map(|(key, label)| Category {
        key: key.to_string(),
        label: label.to_string(),
        bar_override: None,
    })
    .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Baseline {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub notes: String,
    /// category key → the USER's 1-10 ranking of this reference.
    #[serde(default)]
    pub scores: std::collections::BTreeMap<String, u8>,
    #[serde(default)]
    pub mime_type: String,
    #[serde(default)]
    pub created_at: String,
    /// "image" (user-uploaded screenshot) or "generated" (agent-produced
    /// HTML, stored in `baseline_html/<id>`). Default keeps 0.1.0 data valid.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// Generated baselines: what this iteration changed vs the previous ones
    /// — the directive that graduates into the overall prompt when the user
    /// rates the result high.
    #[serde(default)]
    pub change_prompt: String,
}

fn default_kind() -> String {
    "image".into()
}

impl Baseline {
    /// Mean of the user's category rankings; None while unrated.
    pub fn avg_score(&self) -> Option<f64> {
        if self.scores.is_empty() {
            return None;
        }
        Some(self.scores.values().map(|s| *s as f64).sum::<f64>() / self.scores.len() as f64)
    }
}

// ── Store access (same envelope as the other Rust plugins) ────────────

pub fn store_put(collection: &str, key: &str, data: Value) -> Result<(), String> {
    call_host(
        HostFn::StorePut,
        &json!({ "collection": collection, "key": key, "data": data }),
    )?;
    Ok(())
}

pub fn store_get(collection: &str, key: &str) -> Result<Option<Value>, String> {
    let out = call_host(
        HostFn::StoreGet,
        &json!({ "collection": collection, "key": key }),
    )?;
    match out.get("value") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => Ok(Some(v.clone())),
    }
}

pub fn store_list(collection: &str) -> Result<Vec<(String, Value)>, String> {
    let out = call_host(HostFn::StoreList, &json!({ "collection": collection }))?;
    Ok(out
        .get("items")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|it| {
                    Some((
                        it.get("key")?.as_str()?.to_string(),
                        it.get("value")?.clone(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default())
}

pub fn store_delete(collection: &str, key: &str) {
    let _ = call_host(
        HostFn::StoreDelete,
        &json!({ "collection": collection, "key": key }),
    );
}

/// Host-supplied clock, stored on `timer.tick` (wasm has no time source).
pub fn set_clock(now: &str) {
    let _ = store_put(ENGINE_COLLECTION, "clock", json!({ "now": now }));
}

pub fn clock() -> String {
    store_get(ENGINE_COLLECTION, "clock")
        .ok()
        .flatten()
        .and_then(|v| v.get("now").and_then(|n| n.as_str()).map(str::to_string))
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".into())
}

// ── Categories + bars ─────────────────────────────────────────────────

pub fn categories() -> Vec<Category> {
    store_get(CATEGORIES_COLLECTION, "all")
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_else(default_categories)
}

pub fn save_categories(cats: &[Category]) -> Result<(), String> {
    store_put(
        CATEGORIES_COLLECTION,
        "all",
        serde_json::to_value(cats).map_err(|e| e.to_string())?,
    )
}

pub fn baselines() -> Vec<Baseline> {
    let mut out: Vec<Baseline> = store_list(BASELINES_COLLECTION)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, v)| serde_json::from_value(v).ok())
        .collect();
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    out
}

/// The bar for one category: user override, else the median of the user's
/// baseline rankings for it, else [`DEFAULT_BAR`]. Pure — testable.
pub fn bar_for(cat: &Category, baselines: &[Baseline]) -> u8 {
    if let Some(b) = cat.bar_override {
        return b.clamp(1, 10);
    }
    let mut scores: Vec<u8> = baselines
        .iter()
        .filter_map(|b| b.scores.get(&cat.key).copied())
        .collect();
    if scores.is_empty() {
        return DEFAULT_BAR;
    }
    scores.sort_unstable();
    // Lower median for even counts: the stricter of the two middles would
    // punish a user whose baselines straddle the bar; the looser one is the
    // honest "half my references are at least this good".
    scores[(scores.len() - 1) / 2]
}

// ── Verdicts ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct Gap {
    pub category: String,
    pub label: String,
    pub score: u8,
    pub bar: u8,
}

/// Compare submitted scores against the bars. Unknown categories are
/// rejected; missing categories are gaps at score 0 (unscored ≠ passed).
pub fn judge(
    scores: &std::collections::BTreeMap<String, u8>,
    cats: &[Category],
    baselines: &[Baseline],
) -> Result<Vec<Gap>, String> {
    for key in scores.keys() {
        if !cats.iter().any(|c| &c.key == key) {
            return Err(format!(
                "unknown category '{key}' — call ui_gauge_rubric for the current category keys"
            ));
        }
    }
    let mut gaps = Vec::new();
    for cat in cats {
        let bar = bar_for(cat, baselines);
        let score = scores.get(&cat.key).copied().unwrap_or(0);
        if score < bar {
            gaps.push(Gap {
                category: cat.key.clone(),
                label: cat.label.clone(),
                score,
                bar,
            });
        }
    }
    Ok(gaps)
}

// ── Evaluation history ────────────────────────────────────────────────

pub fn record_evaluation(eval: Value) -> Result<(), String> {
    let id = eval
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("evaluation missing id")?
        .to_string();
    store_put(EVALUATIONS_COLLECTION, &id, eval)?;
    // Prune beyond the cap, oldest first (keys sort by ts prefix).
    let mut keys: Vec<String> = store_list(EVALUATIONS_COLLECTION)?
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    if keys.len() > EVALUATIONS_CAP {
        keys.sort();
        for k in keys.iter().take(keys.len() - EVALUATIONS_CAP) {
            store_delete(EVALUATIONS_COLLECTION, k);
        }
    }
    Ok(())
}

pub fn evaluations(target: Option<&str>) -> Vec<Value> {
    let mut evals: Vec<Value> = store_list(EVALUATIONS_COLLECTION)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, v)| v)
        .filter(|v| match target {
            Some(t) => v.get("target").and_then(|x| x.as_str()) == Some(t),
            None => true,
        })
        .collect();
    evals.sort_by(|a, b| {
        let ta = a.get("ts").and_then(|v| v.as_str()).unwrap_or("");
        let tb = b.get("ts").and_then(|v| v.as_str()).unwrap_or("");
        tb.cmp(ta) // newest first
    });
    evals
}

/// Monotonic-enough id: clock second + atomic counter (sorts by time).
pub fn new_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let compact: String = clock().chars().filter(|c| c.is_ascii_digit()).collect();
    format!("{prefix}-{compact}-{n:04}")
}

// ── Overall baseline prompt + the generation loop (0.2.0) ──────────

/// A generated baseline whose average user rating clears this graduates its
/// `change_prompt` into the overall prompt.
pub const HIGH_AVG: f64 = 7.0;
/// At or below this, the change is listed as something to avoid when
/// generating the next iteration.
pub const LOW_AVG: f64 = 4.0;
/// Generated HTML cap — stays under the 256 KB store-document ceiling.
pub const HTML_MAX_LEN: usize = 180_000;

/// The living "overall baseline prompt": the style directives the user has
/// validated, assembled from every high-rated generated baseline's
/// `change_prompt` (oldest → newest, so later refinements read last). Pure
/// and computed on read — it can never go stale: re-rating or deleting a
/// baseline changes the very next read.
pub fn overall_prompt(baselines: &[Baseline]) -> String {
    let mut proven: Vec<String> = Vec::new();
    for b in baselines {
        if b.kind != "generated" || b.change_prompt.trim().is_empty() {
            continue;
        }
        if let Some(avg) = b.avg_score()
            && avg >= HIGH_AVG
        {
            proven.push(format!("- [rated {avg:.1}/10] {}", b.change_prompt.trim()));
        }
    }
    let mut out = String::from(
        "# Overall UI baseline prompt\n\
         Style directives the user has validated by rating generated baselines \
         7+/10. Apply ALL of them when designing or judging UI for this user.\n",
    );
    if proven.is_empty() {
        out.push_str("\n(No validated directives yet — generate baselines and rate them.)\n");
    } else {
        out.push('\n');
        out.push_str(&proven.join("\n"));
        out.push('\n');
    }
    out
}

/// The prompt handed to a generation session: the validated overall prompt,
/// what to improve on, what to avoid, and the submission contract.
pub fn build_generation_prompt(baselines: &[Baseline]) -> String {
    let generated: Vec<&Baseline> = baselines.iter().filter(|b| b.kind == "generated").collect();
    let mut keep = Vec::new();
    let mut avoid = Vec::new();
    let mut unrated = 0usize;
    for b in generated.iter().rev().take(10) {
        match b.avg_score() {
            Some(avg) if avg >= HIGH_AVG => {
                keep.push(format!("- [{avg:.1}/10] {}", b.change_prompt.trim()))
            }
            Some(avg) if avg <= LOW_AVG => {
                avoid.push(format!("- [{avg:.1}/10] {}", b.change_prompt.trim()))
            }
            Some(_) => {}
            None => unrated += 1,
        }
    }
    let mut p = String::from(
        "Generate ONE self-contained baseline UI page: a realistic, polished dashboard-style \
         screen (invent plausible content) as a single HTML document with ALL CSS inlined in a \
         <style> tag. No external resources, no JavaScript — it is rendered statically in a \
         sandboxed frame for the user to rate 1-10 per design category.\n\n",
    );
    p.push_str(&overall_prompt(baselines));
    if !avoid.is_empty() {
        p.push_str("\n## The user rated these changes LOW — avoid repeating them\n");
        p.push_str(&avoid.join("\n"));
        p.push('\n');
    }
    if unrated > 0 {
        p.push_str(&format!(
            "\n({unrated} earlier generated baseline(s) are still unrated — vary a different \
             aspect rather than re-testing the same idea.)\n"
        ));
    }
    let _ = keep; // already embedded via overall_prompt
    p.push_str(
        "\n## Your task\n\
         1. Improve on the previous baselines: keep every validated directive above, then \
         change or refine ONE clear aspect (layout rhythm, type scale, color system, density, \
         component styling, …) that could raise the user's ratings.\n\
         2. Submit EXACTLY ONE result by calling the ui_gauge_submit_baseline tool with:\n\
         - html: the complete HTML document (≤ 180000 chars)\n\
         - change_summary: one short paragraph of WHAT you changed vs the previous baselines \
         and why — written as a reusable style directive (it becomes part of the overall \
         prompt if the user rates it high)\n\
         - name: a short title for this iteration\n\
         Do not ask questions; do not produce anything else.",
    );
    p
}

// ── Generation state (engine/generation) ────────────────────────

pub fn generation_state() -> Value {
    store_get(ENGINE_COLLECTION, "generation")
        .ok()
        .flatten()
        .unwrap_or_else(|| json!({ "status": "idle" }))
}

pub fn set_generation_state(v: Value) {
    let _ = store_put(ENGINE_COLLECTION, "generation", v);
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn cat(key: &str, over: Option<u8>) -> Category {
        Category {
            key: key.into(),
            label: key.into(),
            bar_override: over,
        }
    }
    fn base(scores: &[(&str, u8)]) -> Baseline {
        Baseline {
            id: "b".into(),
            name: "b".into(),
            notes: String::new(),
            scores: scores.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            mime_type: "image/png".into(),
            created_at: String::new(),
            kind: "image".into(),
            change_prompt: String::new(),
        }
    }

    #[test]
    fn bar_prefers_override_then_median_then_default() {
        let c = cat("typography", Some(9));
        assert_eq!(bar_for(&c, &[]), 9);
        let c = cat("typography", None);
        assert_eq!(bar_for(&c, &[]), DEFAULT_BAR);
        let bs = vec![
            base(&[("typography", 4)]),
            base(&[("typography", 8)]),
            base(&[("typography", 6)]),
        ];
        assert_eq!(bar_for(&c, &bs), 6, "odd count → true median");
        let bs = vec![base(&[("typography", 4)]), base(&[("typography", 8)])];
        assert_eq!(bar_for(&c, &bs), 4, "even count → lower median");
    }

    #[test]
    fn judge_flags_below_bar_missing_and_unknown() {
        let cats = vec![cat("a", Some(7)), cat("b", Some(5))];
        let mut scores = BTreeMap::new();
        scores.insert("a".to_string(), 8u8);
        // "b" missing → gap at 0.
        let gaps = judge(&scores, &cats, &[]).unwrap();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].category, "b");
        assert_eq!(gaps[0].score, 0);
        scores.insert("b".to_string(), 5u8);
        assert!(
            judge(&scores, &cats, &[]).unwrap().is_empty(),
            "at bar passes"
        );
        scores.insert("zzz".to_string(), 3u8);
        assert!(
            judge(&scores, &cats, &[]).is_err(),
            "unknown category rejected"
        );
    }

    #[test]
    fn default_categories_cover_the_six() {
        let cats = default_categories();
        assert_eq!(cats.len(), 6);
        assert!(cats.iter().any(|c| c.key == "accessibility"));
    }

    fn generated(change: &str, scores: &[(&str, u8)], ts: &str) -> Baseline {
        Baseline {
            id: ts.into(),
            name: "gen".into(),
            notes: String::new(),
            scores: scores.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            mime_type: String::new(),
            created_at: ts.into(),
            kind: "generated".into(),
            change_prompt: change.into(),
        }
    }

    #[test]
    fn avg_score_none_until_rated() {
        let b = generated("x", &[], "1");
        assert!(b.avg_score().is_none());
        let b = generated("x", &[("a", 6), ("b", 9)], "1");
        assert_eq!(b.avg_score(), Some(7.5));
    }

    #[test]
    fn overall_prompt_keeps_only_high_rated_generated_changes() {
        let bs = vec![
            generated("tighter spacing", &[("a", 8), ("b", 8)], "1"), // 8.0 → in
            generated("neon palette", &[("a", 3), ("b", 4)], "2"),    // 3.5 → out
            generated("bigger type scale", &[], "3"),                 // unrated → out
            base(&[("a", 10)]),                                       // image kind → out
        ];
        let p = overall_prompt(&bs);
        assert!(p.contains("tighter spacing"), "{p}");
        assert!(p.contains("[rated 8.0/10]"), "{p}");
        assert!(!p.contains("neon palette"), "{p}");
        assert!(!p.contains("bigger type scale"), "{p}");
    }

    #[test]
    fn overall_prompt_updates_when_a_rating_changes() {
        let mut bs = vec![generated("card shadows", &[("a", 5)], "1")];
        assert!(!overall_prompt(&bs).contains("card shadows"));
        bs[0].scores.insert("a".into(), 9);
        assert!(
            overall_prompt(&bs).contains("card shadows"),
            "re-rating must refresh"
        );
        bs.clear();
        assert!(overall_prompt(&bs).contains("No validated directives"));
    }

    #[test]
    fn generation_prompt_carries_overall_avoid_and_contract() {
        let bs = vec![
            generated("tighter spacing", &[("a", 8)], "1"),
            generated("neon palette", &[("a", 2)], "2"),
            generated("glass morphism", &[], "3"),
        ];
        let p = build_generation_prompt(&bs);
        assert!(p.contains("tighter spacing"), "{p}");
        assert!(p.contains("avoid repeating"), "{p}");
        assert!(p.contains("neon palette"), "{p}");
        assert!(p.contains("still unrated"), "{p}");
        assert!(p.contains("ui_gauge_submit_baseline"), "{p}");
        assert!(p.contains("change_summary"), "{p}");
    }
}
