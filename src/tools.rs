//! The MCP tools: submit a generated page, read the user's UI preference
//! prompt, fetch a starred-element reference screenshot, and page history.

use serde_json::{Value, json};

use crate::gauge::{self, Feedback, Page, PageElement};
use crate::host::{HostFn, call_host};

fn require_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("'{key}' is required"))
}

fn shot_keys() -> std::collections::BTreeSet<String> {
    gauge::store_list(gauge::SHOTS_COLLECTION)
        .unwrap_or_default()
        .into_iter()
        .map(|(k, _)| k)
        .collect()
}

/// `ui_gauge_submit_page { html, elements, name, design_notes? }` — the
/// generation session submits one marked-up page. Stored unreviewed; the
/// user gives per-element feedback on the UI Gauge page, which feeds the
/// preference prompt. Completes the page's pending generation when the
/// caller is that generation's session.
pub fn submit_page_tool(args: Value) -> Result<Value, String> {
    let html = require_str(&args, "html")?;
    let name = require_str(&args, "name")?;
    let design_notes = args
        .get("design_notes")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let elements: Vec<PageElement> = serde_json::from_value(
        args.get("elements")
            .cloned()
            .ok_or("'elements' is required: [{id, label, kind?}, ...]")?,
    )
    .map_err(|e| format!("bad elements: {e}"))?;
    gauge::validate_submission(&html, &elements)?;

    // The pending generation's brief/model belong on the stored page.
    let generation = gauge::generation_state();
    let pending_session = generation
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let caller = call_host(HostFn::CallerScope, &json!({}))
        .ok()
        .and_then(|s| {
            s.get("session_id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    let is_pending = !pending_session.is_empty() && pending_session == caller;

    let id = gauge::new_id("page");
    let page = Page {
        id: id.clone(),
        name: name.trim().to_string(),
        brief: if is_pending {
            generation
                .get("brief")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        } else {
            String::new()
        },
        model: if is_pending {
            generation
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        } else {
            String::new()
        },
        design_notes,
        created_at: gauge::clock(),
        elements,
    };
    gauge::store_put(
        gauge::PAGES_COLLECTION,
        &id,
        serde_json::to_value(&page).map_err(|e| e.to_string())?,
    )?;
    gauge::store_put(gauge::PAGE_HTML_COLLECTION, &id, json!({ "html": html }))?;

    // Prune beyond the cap, oldest first (page ids sort by clock prefix).
    let mut keys: Vec<String> = gauge::store_list(gauge::PAGES_COLLECTION)?
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    if keys.len() > gauge::PAGES_CAP {
        keys.sort();
        for k in keys.iter().take(keys.len() - gauge::PAGES_CAP) {
            gauge::delete_page(k);
        }
    }

    if is_pending {
        gauge::set_generation_state(json!({
            "status": "done",
            "session_id": pending_session,
            "page_id": id,
            "finished_at": gauge::clock(),
        }));
    }
    Ok(json!({
        "ok": true,
        "page_id": id,
        "note": "stored; awaiting the user's per-element feedback on the UI Gauge page",
    }))
}

/// `ui_gauge_prefs {}` — the composed UI preference prompt plus the starred
/// reference list, for agents that want the taste profile explicitly
/// (sessions in an enabled folder already receive it as a system-prompt
/// block).
pub fn prefs_tool(_args: Value) -> Result<Value, String> {
    let pages = gauge::pages();
    let feedback = gauge::all_feedback();
    let pref = gauge::preference_prompt(&pages, &feedback, &shot_keys());
    let references: Vec<Value> = pref
        .ingredients
        .iter()
        .filter(|i| i.section == "reference")
        .map(|i| {
            json!({
                "id": gauge::feedback_key(&i.page_id, &i.element_id),
                "label": i.element_label,
                "page": i.page_name,
            })
        })
        .collect();
    Ok(json!({
        "prompt": pref.text,
        "references": references,
        "instructions": "Apply every Do/Avoid line when building or judging UI for this \
    user. Fetch one or two reference screenshots with ui_gauge_reference_image before \
    designing UI similar to a starred element.",
    }))
}

/// `ui_gauge_reference_image { id }` — one starred element's screenshot
/// (base64), id as listed by ui_gauge_prefs / the injected prompt.
pub fn reference_image_tool(args: Value) -> Result<Value, String> {
    let id = require_str(&args, "id")?;
    let img = gauge::store_get(gauge::SHOTS_COLLECTION, &id)?
        .ok_or_else(|| format!("no reference screenshot '{id}' — list ids with ui_gauge_prefs"))?;
    Ok(img)
}

/// `ui_gauge_history {}` — generated pages with their feedback, newest
/// first. Lets an agent see what was tried and how the user reacted.
pub fn history_tool(_args: Value) -> Result<Value, String> {
    let pages = gauge::pages();
    let feedback = gauge::all_feedback();
    let mut out: Vec<Value> = pages
        .iter()
        .rev()
        .take(25)
        .map(|p| {
            let fb: Vec<Value> = feedback
                .iter()
                .filter(|f| f.page_id == p.id)
                .map(|f: &Feedback| {
                    json!({
                        "element_id": f.element_id,
                        "verdict": f.verdict,
                        "comment": f.comment,
                        "starred": f.starred,
                    })
                })
                .collect();
            json!({
                "id": p.id,
                "name": p.name,
                "brief": p.brief,
                "created_at": p.created_at,
                "design_notes": p.design_notes,
                "elements": p.elements,
                "feedback": fb,
            })
        })
        .collect();
    out.shrink_to_fit();
    Ok(json!({ "pages": out }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn require_str_rejects_blank() {
        assert!(require_str(&json!({ "a": " " }), "a").is_err());
        assert!(require_str(&json!({}), "a").is_err());
        assert_eq!(require_str(&json!({ "a": "x" }), "a").unwrap(), "x");
    }
}
