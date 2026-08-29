//! Plugin manifest: identity, hooks, MCP tools, UI surfaces, permissions.

/// Inline SVG (lucide "gauge") for the sidebar entry; rendered sandboxed.
const ICON: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\" fill=\"none\" \
stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\">\
<path d=\"m12 14 4-4\"/><path d=\"M3.34 19a10 10 0 1 1 17.32 0\"/></svg>";

pub fn manifest_json() -> String {
    let manifest = serde_json::json!({
        "description": env!("CARGO_PKG_DESCRIPTION"),
        "version": env!("CARGO_PKG_VERSION"),
        "repository": env!("CARGO_PKG_REPOSITORY"),

        "hooks": [
            "mcp.tool.invoke",
            // Clock only — wasm has no time source; evaluations are stamped
            // with the last tick's timestamp.
            "timer.tick",
            "http.request.before",
            "http.request.authed",
        ],

        "mcp_tools": [
            {
                "name": "ui_gauge_rubric",
                "title": "UI-gauge rubric + calibration anchors",
                "description": "The user's UI design rubric: categories with their 1-10 bars, and the user's ranked baseline screenshots (scores + notes; fetch images with ui_gauge_baseline_image). Call this BEFORE scoring any UI so your numbers sit on the user's calibrated scale, then score the target and submit via ui_gauge_score.",
                "input_schema": { "type": "object", "properties": {}, "required": [], "additionalProperties": false }
            },
            {
                "name": "ui_gauge_baseline_image",
                "title": "Fetch one baseline image",
                "description": "One user-ranked baseline screenshot (base64 + mime type), for calibrating your scoring against the user's scale. Fetch one or two anchors near the bar rather than all of them.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Baseline id from ui_gauge_rubric." }
                    },
                    "required": ["id"],
                    "additionalProperties": false
                }
            },
            {
                "name": "ui_gauge_score",
                "title": "Score a UI against the baselines",
                "description": "Submit your 1-10 per-category scores for a target UI (a page, screen, or change). Every category from ui_gauge_rubric must be scored — unscored counts as 0. Categories below their bar make the verdict 'subpar' and automatically create one follow-up card per gap in the caller's project; the evaluation lands in the UI Gauge page history either way.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "target": { "type": "string", "description": "What was scored, e.g. \"settings page after redesign\"." },
                        "scores": {
                            "type": "object",
                            "description": "category_key → integer 1-10, on the user's calibrated scale.",
                            "additionalProperties": { "type": "integer", "minimum": 1, "maximum": 10 }
                        },
                        "notes": { "type": "string", "description": "Short evaluator notes — what dragged scores down; copied into follow-up cards." }
                    },
                    "required": ["target", "scores"],
                    "additionalProperties": false
                }
            },
            {
                "name": "ui_gauge_history",
                "title": "Past UI-gauge evaluations",
                "description": "Recent evaluations (newest first, max 25), optionally filtered by target substring-exact match. Use it to check whether a re-evaluation improved on the last one.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "target": { "type": "string", "description": "Optional exact target to filter by." }
                    },
                    "required": [],
                    "additionalProperties": false
                }
            }
        ],

        "sidebar_items": [
            { "id": "ui-gauge", "label": "UI Gauge", "icon": ICON, "path": "/plugin-api/v1/ui-gauge" }
        ],
        "http_routes": ["GET /plugin-api/v1/ui-gauge"],
        "ui_routes": [
            "GET /api/plugin-ui/ui-gauge/state",
            "POST /api/plugin-ui/ui-gauge/categories",
            "POST /api/plugin-ui/ui-gauge/baselines",
            "POST /api/plugin-ui/ui-gauge/baselines/:id",
            "POST /api/plugin-ui/ui-gauge/baselines/:id/delete",
            "GET /api/plugin-ui/ui-gauge/baselines/:id/image"
        ],

        "permissions": [
            "provide_mcp_tools",
            "data_store",          // rubric, baselines, evaluation history
            "user_authority",      // authed page routes
            "contribute_sidebar"   // the UI Gauge sidebar entry
        ],
    });
    manifest.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_declares_tools_and_page() {
        let m: serde_json::Value = serde_json::from_str(&manifest_json()).unwrap();
        let tools: Vec<&str> = m["mcp_tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        for t in [
            "ui_gauge_rubric",
            "ui_gauge_baseline_image",
            "ui_gauge_score",
            "ui_gauge_history",
        ] {
            assert!(tools.contains(&t), "missing tool {t}");
        }
        assert_eq!(m["sidebar_items"][0]["path"], "/plugin-api/v1/ui-gauge");
        let hooks: Vec<&str> = m["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|h| h.as_str())
            .collect();
        assert!(hooks.contains(&"timer.tick"));
        assert!(hooks.contains(&"http.request.authed"));
    }
}
