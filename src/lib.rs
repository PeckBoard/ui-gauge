//! Peckboard ui-gauge plugin (WASM / Extism).
//!
//! Gauges UI design quality against **user-ranked baselines**: the user
//! uploads reference screenshots on the UI Gauge page and ranks each 1-10
//! per category (visual hierarchy, spacing, typography, color/contrast,
//! consistency, accessibility — editable). A vision-capable agent then
//! scores worker output on that same calibrated scale
//! (`ui_gauge_rubric` → `ui_gauge_baseline_image` → `ui_gauge_score`); a
//! category below its bar makes the verdict subpar and auto-creates one
//! follow-up card per gap. The plugin owns the rubric, calibration,
//! verdicts, history, and cards — never the pixels.
//!
//! ## Plugin interface
//!
//! Core expects four exports (`peckboard/src/plugin/manager.rs`):
//! `manifest` / `init` / `handle` / `shutdown`.

#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code, unused_imports))]

mod gauge;
mod host;
mod manifest;
mod page;
mod tools;

use serde::Deserialize;

#[cfg(target_arch = "wasm32")]
mod entry {
    use super::*;
    use extism_pdk::*;

    #[plugin_fn]
    pub fn manifest() -> FnResult<String> {
        Ok(crate::manifest::manifest_json())
    }

    #[plugin_fn]
    pub fn init(_config: String) -> FnResult<String> {
        Ok(serde_json::json!({ "ok": true }).to_string())
    }

    #[plugin_fn]
    pub fn shutdown() -> FnResult<String> {
        Ok(serde_json::json!({ "ok": true }).to_string())
    }

    #[plugin_fn]
    pub fn handle(input: String) -> FnResult<String> {
        let call: HookCall = serde_json::from_str(&input)?;
        Ok(dispatch_hook(&call.hook, call.payload))
    }
}

/// The `{ "hook", "payload" }` envelope core passes to `handle`.
#[derive(Debug, Deserialize)]
struct HookCall {
    hook: String,
    #[serde(default)]
    payload: serde_json::Value,
}

fn dispatch_hook(hook: &str, payload: serde_json::Value) -> String {
    match hook {
        "mcp.tool.invoke" => handle_invoke(payload),
        "timer.tick" => {
            // Clock only — evaluations are stamped with the last tick.
            if let Some(now) = payload.get("now").and_then(|v| v.as_str()) {
                gauge::set_clock(now);
            }
            skip()
        }
        // A generation session that ends without ever submitting a baseline
        // flips the pending generation to "ended" so the page can say so
        // (a successful submit already set it to "done").
        "session.agent.ended" => {
            let ended = payload
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let generation = gauge::generation_state();
            if !ended.is_empty()
                && generation.get("status").and_then(|s| s.as_str()) == Some("running")
                && generation.get("session_id").and_then(|s| s.as_str()) == Some(ended)
            {
                gauge::set_generation_state(serde_json::json!({
                    "status": "ended",
                    "session_id": ended,
                    "finished_at": gauge::clock(),
                }));
            }
            skip()
        }
        "http.request.before" => match page::serve_public(payload) {
            Ok(resp) => allow(resp),
            Err(e) => cancel(&e),
        },
        "http.request.authed" => match page::serve_authed(payload) {
            Ok(resp) => allow(resp),
            Err(e) => cancel(&e),
        },
        _ => skip(),
    }
}

fn handle_invoke(payload: serde_json::Value) -> String {
    let tool = payload
        .get("tool")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let args = payload
        .get("arguments")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    let result: Result<serde_json::Value, String> = match tool.as_str() {
        "ui_gauge_rubric" => tools::rubric_tool(args),
        "ui_gauge_baseline_image" => tools::baseline_image_tool(args),
        "ui_gauge_score" => tools::score_tool(args),
        "ui_gauge_history" => tools::history_tool(args),
        "ui_gauge_submit_baseline" => tools::submit_baseline_tool(args),
        other => return cancel(&format!("ui-gauge does not provide tool '{other}'")),
    };

    match result {
        Ok(value) => allow(value),
        Err(reason) => cancel(&reason),
    }
}

fn allow(value: serde_json::Value) -> String {
    serde_json::json!({ "verdict": "allow", "payload": value }).to_string()
}

fn cancel(reason: &str) -> String {
    serde_json::json!({ "verdict": "cancel", "reason": reason }).to_string()
}

fn skip() -> String {
    serde_json::json!({ "verdict": "skip" }).to_string()
}
