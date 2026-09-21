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

        // Up to 4 calls run in parallel on separate wasm instances
        // (peckboard ≥ 0.0.189; older cores ignore this and serialize).
        // Safe because every store read→modify→write holds the
        // cross-instance lease — see `gauge::try_with_store_lock`.
        "concurrency": 4,
        "hooks": [
            "mcp.tool.invoke",
            // Clock only — wasm has no time source; records are stamped
            // with the last tick's timestamp.
            "timer.tick",
            // Marks a generation session that ended without submitting.
            "session.agent.ended",
            // Keeps each chat session's UI-taste block in sync with its
            // folder's toggle (see prompt_sync.rs).
            "session.message.before",
            "http.request.before",
            "http.request.authed",
        ],

        "mcp_tools": [
            {
                "name": "ui_gauge_submit_page",
                "title": "Submit a generated UI page",
                "description": "Submit ONE generated UI page: a complete self-contained HTML document (all CSS inline, no JavaScript, no external resources, max 180000 chars) whose meaningful regions each carry data-uig-id (unique kebab-case) and data-uig-label (short human label) attributes, plus the matching elements list and short design_notes. The user reviews it element by element on the UI Gauge page; their feedback becomes the UI preference prompt future sessions receive. Used by the page's Generate sessions; callable by any agent asked to produce a ui-gauge page.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "html": { "type": "string", "description": "The complete HTML document (inline CSS, no JS, ≤180000 chars, data-uig-id/data-uig-label on every marked element)." },
                        "elements": {
                            "type": "array",
                            "description": "Every marked element, matching the data-uig-id attributes exactly (max 40).",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string", "description": "The element's data-uig-id (kebab-case, unique)." },
                                    "label": { "type": "string", "description": "Short human label, e.g. \"Metric summary cards\"." },
                                    "kind": { "type": "string", "description": "Optional coarse kind: header, nav, card, form, table, …" }
                                },
                                "required": ["id", "label"],
                                "additionalProperties": false
                            }
                        },
                        "name": { "type": "string", "description": "Short title for the page." },
                        "design_notes": { "type": "string", "description": "2-4 sentences on the chosen design direction." }
                    },
                    "required": ["html", "elements", "name"],
                    "additionalProperties": false
                }
            },
            {
                "name": "ui_gauge_prefs",
                "title": "The user's UI preference prompt",
                "description": "The composed UI taste profile: Do/Avoid directives distilled from the user's per-element feedback on generated pages, plus the list of starred reference screenshots (fetch with ui_gauge_reference_image). Sessions in a folder where the user enabled the prompt already receive it automatically; call this when you want it explicitly (e.g. from a worker) or to check the current state before building UI.",
                "input_schema": { "type": "object", "properties": {}, "required": [], "additionalProperties": false }
            },
            {
                "name": "ui_gauge_reference_image",
                "title": "Fetch one UI reference screenshot",
                "description": "One starred element's screenshot (base64 + mime type) — the user marked it as a visual reference for how UI should look. Ids come from ui_gauge_prefs or the injected UI-taste prompt. Fetch one or two relevant references before designing similar UI, not all of them.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Reference id, e.g. \"page-…:metric-cards\"." }
                    },
                    "required": ["id"],
                    "additionalProperties": false
                }
            },
            {
                "name": "ui_gauge_history",
                "title": "Generated pages + user feedback",
                "description": "Generated UI pages (newest first, max 25) with the user's per-element feedback: verdicts, comments, stars. Use it to see what was already tried and how the user reacted before generating or proposing new UI.",
                "input_schema": { "type": "object", "properties": {}, "required": [], "additionalProperties": false }
            }
        ],

        "sidebar_items": [
            { "id": "ui-gauge", "label": "UI Gauge", "icon": ICON, "path": "/plugin-api/v1/ui-gauge" }
        ],
        "http_routes": ["GET /plugin-api/v1/ui-gauge"],
        "ui_routes": [
            "GET /api/plugin-ui/ui-gauge/state",
            "GET /api/plugin-ui/ui-gauge/pickers",
            "POST /api/plugin-ui/ui-gauge/generate",
            "POST /api/plugin-ui/ui-gauge/feedback",
            "POST /api/plugin-ui/ui-gauge/shots",
            "POST /api/plugin-ui/ui-gauge/folders",
            "GET /api/plugin-ui/ui-gauge/pages/:id/html",
            "POST /api/plugin-ui/ui-gauge/pages/:id/delete",
            "GET /api/plugin-ui/ui-gauge/shots/:id"
        ],

        "permissions": [
            "provide_mcp_tools",
            "data_store",            // pages, feedback, screenshots, folder toggles
            "user_authority",        // authed page routes
            "contribute_sidebar",    // the UI Gauge sidebar entry
            "session_write",         // create the temp generation session
            "session_dispatch",      // hand it the generation prompt
            "session_prompt_write",  // attach the UI-taste block to sessions
            "models_read"            // the model picker
        ],
    });
    manifest.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_declares_tools_hooks_and_page() {
        let m: serde_json::Value = serde_json::from_str(&manifest_json()).unwrap();
        let tools: Vec<&str> = m["mcp_tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        for t in [
            "ui_gauge_submit_page",
            "ui_gauge_prefs",
            "ui_gauge_reference_image",
            "ui_gauge_history",
        ] {
            assert!(tools.contains(&t), "missing tool {t}");
        }
        assert_eq!(tools.len(), 4, "old scoring tools must be gone");
        assert_eq!(m["sidebar_items"][0]["path"], "/plugin-api/v1/ui-gauge");
        let hooks: Vec<&str> = m["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|h| h.as_str())
            .collect();
        assert!(hooks.contains(&"session.message.before"));
        assert!(hooks.contains(&"timer.tick"));
        assert!(hooks.contains(&"http.request.authed"));
        let perms: Vec<&str> = m["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p.as_str())
            .collect();
        assert!(perms.contains(&"session_prompt_write"));
    }
}
