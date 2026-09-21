//! The UI Gauge page: a public shell route serving the HTML, and authed
//! JSON routes under `/api/plugin-ui/ui-gauge` the page calls through the
//! parent frame's fetch bridge.
//!
//! The page renders each generated page into a **sanitized shadow root**:
//! the HTML is parsed with `DOMParser` (an inert document — nothing
//! executes), `<script>` elements, `on*` attributes, and `javascript:`
//! URLs are stripped, and the result is adopted into a shadow DOM. An
//! iframe cannot work here: the plugin page itself runs in a sandboxed
//! frame without `allow-same-origin`, and sandbox flags inherit, so a
//! nested frame's document would be cross-origin and unmeasurable — the
//! review overlay needs `[data-uig-id]` rects and the starred-element
//! screenshot capture needs the live nodes.

use serde_json::{Value, json};

use crate::gauge::{self, Feedback, PageElement};
use crate::host::{HostFn, call_host};

pub const PAGE_PATH: &str = "/plugin-api/v1/ui-gauge";
const API_PREFIX: &str = "/api/plugin-ui/ui-gauge";

/// Screenshot cap: base64 stays under the 256 KB store-document ceiling
/// with headroom for the JSON envelope.
const MAX_IMAGE_BASE64_LEN: usize = 200_000;

pub fn serve_public(payload: Value) -> Result<Value, String> {
    let path = payload.get("path").and_then(|v| v.as_str()).unwrap_or("");
    if path == PAGE_PATH {
        Ok(json!({
            "status": 200,
            "headers": { "content-type": "text/html; charset=utf-8" },
            "body": PAGE_HTML,
        }))
    } else {
        Ok(json!({
            "status": 404,
            "headers": { "content-type": "text/html; charset=utf-8" },
            "body": "<!doctype html><h1>404</h1>",
        }))
    }
}

fn json_response(status: u16, body: Value) -> Value {
    json!({
        "status": status,
        "headers": { "content-type": "application/json" },
        "body": body.to_string(),
    })
}

pub fn serve_authed(payload: Value) -> Result<Value, String> {
    let method = payload
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("GET")
        .to_uppercase();
    let path = payload.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let body: Value = payload
        .get("body")
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);

    let rest = path.strip_prefix(API_PREFIX).unwrap_or("");
    let segs: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();

    // Mutating routes with a read→modify→write hold the cross-instance
    // store lease (manifest `concurrency` > 1 runs calls on several wasm
    // instances). Contended → a "busy" banner.
    let locked = |f: &dyn Fn() -> Result<Value, String>| -> Result<Value, String> {
        gauge::try_with_store_lock(f)?.ok_or_else(|| gauge::BUSY_MSG.to_string())
    };
    let out = match (method.as_str(), segs.as_slice()) {
        ("GET", ["state"]) => state_route(),
        ("GET", ["pickers"]) => pickers_route(),
        ("POST", ["generate"]) => locked(&|| generate_route(&body)),
        ("POST", ["feedback"]) => locked(&|| feedback_route(&body)),
        ("POST", ["shots"]) => shots_route(&body),
        ("POST", ["folders"]) => folders_route(&body),
        ("GET", ["pages", id, "html"]) => {
            match gauge::store_get(gauge::PAGE_HTML_COLLECTION, id)? {
                Some(html) => Ok(json_response(200, html)),
                None => Ok(json_response(404, json!({ "error": "no html" }))),
            }
        }
        ("POST", ["pages", id, "delete"]) => {
            gauge::delete_page(id);
            Ok(json_response(200, json!({ "ok": true })))
        }
        ("GET", ["shots", id]) => match gauge::store_get(gauge::SHOTS_COLLECTION, id)? {
            Some(img) => Ok(json_response(200, img)),
            None => Ok(json_response(404, json!({ "error": "no shot" }))),
        },
        _ => Ok(json_response(404, json!({ "error": "no such route" }))),
    };
    Ok(out.unwrap_or_else(|e| json_response(400, json!({ "error": e }))))
}

/// Dropdown data for the generation controls; failures degrade to empty
/// lists so one missing grant never blanks the page.
fn pickers_route() -> Result<Value, String> {
    let folders = call_host(HostFn::ListFolders, &json!({}))
        .ok()
        .and_then(|v| v.get("folders").cloned())
        .unwrap_or(json!([]));
    let models = call_host(HostFn::ListModels, &json!({}))
        .ok()
        .and_then(|v| v.get("models").cloned())
        .unwrap_or(json!([]));
    Ok(json_response(
        200,
        json!({ "folders": folders, "models": models }),
    ))
}

fn state_route() -> Result<Value, String> {
    let pages = gauge::pages();
    let feedback = gauge::all_feedback();
    let shot_keys: std::collections::BTreeSet<String> = gauge::store_list(gauge::SHOTS_COLLECTION)?
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    let pref = gauge::preference_prompt(&pages, &feedback, &shot_keys);

    let page_views: Vec<Value> = pages
        .iter()
        .map(|p| {
            let mut fb_map = serde_json::Map::new();
            for f in feedback.iter().filter(|f| f.page_id == p.id) {
                let key = gauge::feedback_key(&f.page_id, &f.element_id);
                fb_map.insert(
                    f.element_id.clone(),
                    json!({
                        "verdict": f.verdict,
                        "comment": f.comment,
                        "starred": f.starred,
                        "star_dismissed": f.star_dismissed,
                        "has_shot": shot_keys.contains(&key),
                    }),
                );
            }
            json!({
                "id": p.id,
                "name": p.name,
                "brief": p.brief,
                "model": p.model,
                "design_notes": p.design_notes,
                "created_at": p.created_at,
                "elements": p.elements,
                "feedback": fb_map,
            })
        })
        .collect();

    // Folder list with each folder's toggle; ListFolders degrades to empty.
    let prefs: std::collections::BTreeMap<String, bool> =
        gauge::store_list(gauge::FOLDER_PREFS_COLLECTION)?
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    v.get("enabled").and_then(|e| e.as_bool()).unwrap_or(false),
                )
            })
            .collect();
    let folders: Vec<Value> = call_host(HostFn::ListFolders, &json!({}))
        .ok()
        .and_then(|v| v.get("folders").cloned())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|f| {
            let id = f.get("id")?.as_str()?.to_string();
            let name = f.get("name").and_then(|n| n.as_str()).unwrap_or(&id);
            Some(json!({
                "id": id,
                "name": name,
                "enabled": prefs.get(&id).copied().unwrap_or(false),
            }))
        })
        .collect();

    Ok(json_response(
        200,
        json!({
            "pages": page_views,
            "generation": gauge::generation_state(),
            "folders": folders,
            "prompt": { "text": pref.text, "ingredients": pref.ingredients },
        }),
    ))
}

/// The button: spawn a temp generation session in the chosen folder with
/// the chosen model, hand it the taste-aware prompt, and record the pending
/// generation. The session submits back via `ui_gauge_submit_page`.
fn generate_route(body: &Value) -> Result<Value, String> {
    let generation = gauge::generation_state();
    if generation.get("status").and_then(|s| s.as_str()) == Some("running") {
        return Err(
            "a generation is already running — wait for it to submit (or for its session \
             to end) before starting another"
                .into(),
        );
    }
    let folder_id = body
        .get("folder_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or("'folder_id' is required")?;
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or("'model' is required — pick the model that generates the page")?;
    let brief = body
        .get("brief")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    // create_session's authed path takes a folder *path*; resolve the picked id.
    let folders = call_host(HostFn::ListFolders, &json!({}))?;
    let folder_path = folders
        .get("folders")
        .and_then(|v| v.as_array())
        .and_then(|fs| {
            fs.iter()
                .find(|f| f.get("id").and_then(|i| i.as_str()) == Some(folder_id))
        })
        .and_then(|f| f.get("path").and_then(|p| p.as_str()))
        .ok_or_else(|| format!("folder not found: {folder_id}"))?
        .to_string();

    let pages = gauge::pages();
    let feedback = gauge::all_feedback();
    let iteration = pages.len() + 1;
    let created = call_host(
        HostFn::CreateSession,
        &json!({
            "name": format!("UI Gauge page #{iteration}"),
            "model": model,
            "is_temp": true,
            "folder_path": folder_path,
            "system_prompt": "You generate one UI page for the ui-gauge plugin and submit \
        it via the ui_gauge_submit_page MCP tool. Follow the task prompt exactly; \
        do not ask the user questions.",
        }),
    )?;
    let session_id = created
        .get("session")
        .and_then(|s| s.get("id"))
        .and_then(|v| v.as_str())
        .ok_or("create_session returned no session id")?
        .to_string();

    call_host(
        HostFn::DispatchCapture,
        &json!({
            "session_id": session_id,
            "prompt": gauge::build_generation_prompt(&brief, &pages, &feedback),
        }),
    )?;
    gauge::set_generation_state(json!({
        "status": "running",
        "session_id": session_id,
        "model": model,
        "brief": brief,
        "started_at": gauge::clock(),
    }));
    Ok(json_response(
        200,
        json!({ "ok": true, "session_id": session_id }),
    ))
}

/// Look up a page and check the element id belongs to it (or is the
/// page-level pseudo element).
fn checked_element(page_id: &str, element_id: &str) -> Result<(), String> {
    let page: gauge::Page = gauge::store_get(gauge::PAGES_COLLECTION, page_id)?
        .and_then(|v| serde_json::from_value(v).ok())
        .ok_or_else(|| format!("no page '{page_id}'"))?;
    if element_id != gauge::PAGE_ELEMENT_ID
        && !page
            .elements
            .iter()
            .any(|e: &PageElement| e.id == element_id)
    {
        return Err(format!("page '{page_id}' has no element '{element_id}'"));
    }
    Ok(())
}

/// Partial update of one element's feedback: only the keys present in the
/// body change; the rest carries over.
fn feedback_route(body: &Value) -> Result<Value, String> {
    let page_id = body
        .get("page_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("'page_id' is required")?;
    let element_id = body
        .get("element_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("'element_id' is required")?;
    checked_element(page_id, element_id)?;

    let key = gauge::feedback_key(page_id, element_id);
    let mut fb: Feedback = gauge::store_get(gauge::FEEDBACK_COLLECTION, &key)?
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    fb.page_id = page_id.to_string();
    fb.element_id = element_id.to_string();
    if let Some(v) = body.get("verdict").and_then(|v| v.as_str()) {
        if !["up", "down", ""].contains(&v) {
            return Err("'verdict' must be \"up\", \"down\", or \"\"".into());
        }
        fb.verdict = v.to_string();
    }
    if let Some(c) = body.get("comment").and_then(|v| v.as_str()) {
        fb.comment = c.chars().take(2000).collect();
    }
    if let Some(s) = body.get("starred").and_then(|v| v.as_bool()) {
        fb.starred = s;
    }
    if let Some(d) = body.get("star_dismissed").and_then(|v| v.as_bool()) {
        fb.star_dismissed = d;
    }
    fb.updated_at = gauge::clock();
    gauge::store_put(
        gauge::FEEDBACK_COLLECTION,
        &key,
        serde_json::to_value(&fb).map_err(|e| e.to_string())?,
    )?;
    Ok(json_response(
        200,
        json!({ "ok": true, "feedback": serde_json::to_value(&fb).map_err(|e| e.to_string())? }),
    ))
}

/// Store a captured element screenshot (the page captures client-side and
/// downscales before posting).
fn shots_route(body: &Value) -> Result<Value, String> {
    let page_id = body
        .get("page_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("'page_id' is required")?;
    let element_id = body
        .get("element_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("'element_id' is required")?;
    checked_element(page_id, element_id)?;
    let image_base64 = body
        .get("image_base64")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("'image_base64' is required")?;
    if image_base64.len() > MAX_IMAGE_BASE64_LEN {
        return Err(format!(
            "screenshot too large ({} chars base64, max {MAX_IMAGE_BASE64_LEN})",
            image_base64.len()
        ));
    }
    let mime_type = body
        .get("mime_type")
        .and_then(|v| v.as_str())
        .unwrap_or("image/jpeg");
    gauge::store_put(
        gauge::SHOTS_COLLECTION,
        &gauge::feedback_key(page_id, element_id),
        json!({
            "image_base64": image_base64,
            "mime_type": mime_type,
            "captured_at": gauge::clock(),
        }),
    )?;
    Ok(json_response(200, json!({ "ok": true })))
}

/// Toggle the preference prompt for a folder. Sessions pick the change up
/// on their next turn (`session.message.before` syncs the block).
fn folders_route(body: &Value) -> Result<Value, String> {
    let folder_id = body
        .get("folder_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("'folder_id' is required")?;
    let enabled = body
        .get("enabled")
        .and_then(|v| v.as_bool())
        .ok_or("'enabled' (boolean) is required")?;
    gauge::store_put(
        gauge::FOLDER_PREFS_COLLECTION,
        folder_id,
        json!({ "enabled": enabled }),
    )?;
    Ok(json_response(200, json!({ "ok": true })))
}

const PAGE_HTML: &str = r##"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<title>UI Gauge</title>
<style>
  :root {
    color-scheme: light dark;
    --bg: #f5f6f8; --card: #ffffff; --text: #1c1e21; --muted: #667085;
    --line: #e4e7ec; --accent: #4f6bed; --ok: #12805c; --bad: #b42318; --warn: #b54708;
  }
  @media (prefers-color-scheme: dark) {
    :root { --bg: #101418; --card: #1a2027; --text: #e6e9ee; --muted: #98a2b3;
            --line: #2c3540; --accent: #7c93f5; --ok: #3ccb9a; --bad: #f97066; --warn: #f7b26a; }
  }
  * { box-sizing: border-box; }
  body { margin: 0; padding: 16px; background: var(--bg); color: var(--text);
         font: 14px/1.45 system-ui, sans-serif; }
  h1 { font-size: 18px; margin: 0 0 12px; }
  h2 { font-size: 15px; margin: 0 0 8px; }
  .card { background: var(--card); border: 1px solid var(--line); border-radius: 12px;
          padding: 14px; margin-bottom: 14px; }
  button { font: inherit; padding: 6px 12px; border-radius: 8px; border: 1px solid var(--line);
           background: var(--card); color: var(--text); cursor: pointer; }
  button.primary { background: var(--accent); border-color: var(--accent); color: #fff; }
  button.danger { color: var(--bad); }
  button.active { border-color: var(--accent); color: var(--accent); }
  button:disabled { opacity: .5; cursor: default; }
  input[type=text], textarea, select { padding: 6px 8px; border: 1px solid var(--line);
    border-radius: 8px; background: var(--bg); color: var(--text); font: inherit; }
  .muted { color: var(--muted); font-size: 12px; }
  .error-banner { color: var(--bad); margin: 8px 0; }
  .row { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
  .chip { display: inline-block; padding: 1px 9px; border-radius: 999px; font-size: 12px;
          border: 1px solid var(--line); color: var(--muted); }
  .chip.ok { color: var(--ok); border-color: var(--ok); }
  .chip.warn { color: var(--warn); border-color: var(--warn); }
  pre.prompt { white-space: pre-wrap; background: var(--bg); border: 1px solid var(--line);
    border-radius: 8px; padding: 10px; font-size: 12px; max-height: 320px; overflow: auto; }
  .folder-row { display: flex; gap: 10px; align-items: center; padding: 6px 0;
    border-bottom: 1px solid var(--line); }
  .folder-row:last-child { border-bottom: 0; }
  .pages { display: grid; grid-template-columns: repeat(auto-fill, minmax(260px, 1fr)); gap: 12px; }
  .page-card { border: 1px solid var(--line); border-radius: 10px; padding: 10px; }
  .page-card.open { border-color: var(--accent); }
  .review { display: flex; gap: 12px; align-items: flex-start; }
  .review-doc { flex: 1 1 60%; position: relative; min-width: 0; }
  #review-host { border: 1px solid var(--line); border-radius: 8px; background: #fff;
    overflow: hidden; }
  .overlay { position: absolute; inset: 0; pointer-events: none; }
  .pin { position: absolute; pointer-events: auto; cursor: pointer; min-width: 20px; height: 20px;
    padding: 0 5px; border-radius: 10px; background: var(--accent); color: #fff; font-size: 12px;
    line-height: 20px; text-align: center; border: 2px solid #fff; box-shadow: 0 1px 4px rgba(0,0,0,.35);
    transform: translate(-6px, -6px); }
  .pin.done { background: var(--ok); }
  .pin.down { background: var(--bad); }
  .hl { position: absolute; border: 2px solid var(--accent); border-radius: 6px;
    pointer-events: none; display: none; }
  .rail { flex: 1 1 40%; max-width: 400px; max-height: 80vh; overflow: auto; }
  .el { border: 1px solid var(--line); border-radius: 10px; padding: 8px 10px; margin-bottom: 8px; }
  .el.sel { border-color: var(--accent); }
  .el textarea { width: 100%; min-height: 44px; margin-top: 6px; }
  .star { color: var(--warn); }
</style>
</head>
<body>
<h1>UI Gauge</h1>
<div class="row" style="margin-bottom:12px">
  <button id="refresh-btn" data-testid="gauge-refresh">Refresh</button>
  <span id="stale" class="chip warn" style="display:none" data-testid="gauge-stale">data changed elsewhere — press Refresh when ready</span>
</div>
<div id="banner" class="error-banner" style="display:none"></div>

<div class="card">
  <h2>Generate a page</h2>
  <div class="muted">Spawns a temp agent session that designs one HTML page with every element
  marked for review. It applies all the preferences you have recorded so far. Leave the brief
  empty to let the agent pick a representative page type.</div>
  <div class="row" style="margin-top:8px">
    <select id="g-folder" style="flex:1" data-testid="gauge-gen-folder"></select>
    <select id="g-model" style="flex:1" data-testid="gauge-gen-model"></select>
  </div>
  <div class="row" style="margin-top:8px">
    <input type="text" id="g-brief" placeholder="Optional brief, e.g. 'a settings page' — empty = agent's pick" style="flex:1" data-testid="gauge-gen-brief">
    <button class="primary" id="g-btn" data-testid="gauge-generate">Generate page</button>
  </div>
  <div id="g-status" class="muted" style="margin-top:6px" data-testid="gauge-gen-status"></div>
</div>

<div class="card">
  <h2>Preference prompt</h2>
  <div class="muted">Composed from your per-element feedback — this exact text is attached to
  chat sessions in every folder you enable below (workers are not covered by the hook).
  Starred elements are listed as visual references agents fetch with ui_gauge_reference_image.</div>
  <pre class="prompt" id="prompt" data-testid="gauge-prompt"></pre>
  <details>
    <summary class="muted">What feeds each line</summary>
    <div id="ingredients" data-testid="gauge-ingredients"></div>
  </details>
  <div class="row" style="margin-top:8px">
    <button id="copy-prompt" data-testid="gauge-copy-prompt">Copy prompt</button>
    <span id="copy-done" class="chip ok" style="display:none">copied</span>
  </div>
  <h2 style="margin-top:14px">Folders</h2>
  <div id="folders" data-testid="gauge-folders"></div>
</div>

<div class="card">
  <h2>Generated pages</h2>
  <div id="pages" class="pages"></div>
</div>

<div class="card" id="review-card" style="display:none">
  <div class="row" style="justify-content:space-between">
    <h2 id="review-title"></h2>
    <button id="review-close">Close review</button>
  </div>
  <div id="review-notes" class="muted" style="margin-bottom:8px"></div>
  <div class="review" data-testid="gauge-review">
    <div class="review-doc" id="review-doc">
      <div id="review-host"></div>
      <div class="overlay" id="overlay"></div>
      <div class="hl" id="hl"></div>
    </div>
    <div class="rail" id="rail"></div>
  </div>
</div>

<script>
"use strict";
let seq = 1;
const pending = {};
window.addEventListener("message", (e) => {
  const m = e.data;
  if (!m || m.type !== "plugin-ui-fetch-result") return;
  const cb = pending[m.requestId];
  if (!cb) return;
  delete pending[m.requestId];
  cb(m);
});
function api(method, path, body) {
  return new Promise((resolve, reject) => {
    const id = seq++;
    pending[id] = (m) => {
      let data = null;
      try { data = JSON.parse(m.body); } catch (_) {}
      if (m.status >= 200 && m.status < 300 && !(data && data.error)) resolve(data);
      else reject(new Error((data && data.error) || ("HTTP " + m.status)));
    };
    parent.postMessage({
      type: "plugin-ui-fetch", requestId: id, method, path,
      body: body === undefined ? undefined : JSON.stringify(body),
    }, "*");
  });
}
const BASE = "/api/plugin-ui/ui-gauge";
const esc = (s) => String(s == null ? "" : s).replace(/[&<>"']/g,
  (c) => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]));

let STATE = { pages: [], generation: { status: "idle" }, folders: [], prompt: { text: "", ingredients: [] } };
let PICKERS = { folders: [], models: [] };
let OPEN = null;          // page id under review
let SHADOW = null;        // shadow root holding the sanitized rendered page
const htmlCache = {};

function banner(msg) {
  const el = document.getElementById("banner");
  el.style.display = msg ? "" : "none";
  el.textContent = msg || "";
}
function openPage() { return STATE.pages.find((p) => p.id === OPEN) || null; }
function fb(page, elId) { return (page.feedback || {})[elId] || {}; }

function renderGenerate() {
  const f = document.getElementById("g-folder");
  const m = document.getElementById("g-model");
  if (f.options.length <= 1 && PICKERS.folders.length) {
    f.innerHTML = '<option value="">— folder for the generation session —</option>' +
      PICKERS.folders.map((x) => '<option value="' + esc(x.id) + '">' + esc(x.name) + '</option>').join("");
  }
  if (m.options.length <= 1 && PICKERS.models.length) {
    m.innerHTML = '<option value="">— model —</option>' +
      PICKERS.models.map((x) => { const id = x.id || x.model_id || ""; return '<option value="' + esc(id) + '">' + esc(x.display_name || id) + '</option>'; }).join("");
  }
  const g = STATE.generation || {};
  const st = document.getElementById("g-status");
  document.getElementById("g-btn").disabled = g.status === "running";
  if (g.status === "running") st.textContent = "Generating… the agent session is designing the page (press Refresh to check on it).";
  else if (g.status === "done") st.textContent = "Last generation submitted a page — review it below.";
  else if (g.status === "ended") st.textContent = "The last generation session ended WITHOUT submitting a page — try again (a stronger model helps).";
  else st.textContent = "";
}

function renderPrompt() {
  document.getElementById("prompt").textContent = STATE.prompt.text || "";
  const ing = STATE.prompt.ingredients || [];
  document.getElementById("ingredients").innerHTML = !ing.length
    ? '<div class="muted">Nothing yet — review a generated page below.</div>'
    : ing.map((i) =>
        '<div class="muted" style="margin:3px 0">[' + esc(i.section) + '] ' + esc(i.line) +
        ' <span style="opacity:.7">&larr; ' + esc(i.page_name) + ' &rsaquo; ' + esc(i.element_label) + '</span></div>'
      ).join("");
  document.getElementById("folders").innerHTML = !STATE.folders.length
    ? '<div class="muted">No folders.</div>'
    : STATE.folders.map((f) =>
        '<div class="folder-row" data-testid="gauge-folder-row">' +
        '<label style="flex:1"><input type="checkbox" data-folder="' + esc(f.id) + '"' +
        (f.enabled ? " checked" : "") + ' data-testid="gauge-folder-toggle"> ' + esc(f.name) + '</label>' +
        '<span class="chip' + (f.enabled ? " ok" : "") + '">' +
        (f.enabled ? "prompt attached to this folder's chat sessions" : "off") + '</span>' +
        '</div>'
      ).join("");
}

function renderPages() {
  const el = document.getElementById("pages");
  if (!STATE.pages.length) {
    el.innerHTML = '<div class="muted">No pages yet — press Generate page.</div>';
    return;
  }
  el.innerHTML = STATE.pages.slice().reverse().map((p) => {
    const total = (p.elements || []).length;
    const reviewed = (p.elements || []).filter((e) => {
      const f = fb(p, e.id);
      return f.verdict === "up" || f.verdict === "down" || (f.comment || "").trim();
    }).length;
    const chip = reviewed >= total && total > 0
      ? '<span class="chip ok">reviewed ' + reviewed + "/" + total + '</span>'
      : '<span class="chip' + (reviewed ? "" : " warn") + '">reviewed ' + reviewed + "/" + total + '</span>';
    return '<div class="page-card' + (p.id === OPEN ? " open" : "") + '" data-testid="gauge-page">' +
      '<div class="row" style="justify-content:space-between"><b>' + esc(p.name) + '</b>' + chip + '</div>' +
      (p.brief ? '<div class="muted">brief: ' + esc(p.brief) + '</div>' : "") +
      '<div class="muted">' + esc((p.created_at || "").replace("T", " ").slice(0, 16)) +
      (p.model ? " · " + esc(p.model) : "") + '</div>' +
      '<div class="row" style="margin-top:6px">' +
      '<button class="primary" data-open="' + esc(p.id) + '" data-testid="gauge-open-review">Review</button>' +
      '<button class="danger" data-del="' + esc(p.id) + '" data-testid="gauge-delete-page">Delete</button>' +
      '</div></div>';
  }).join("");
}

// ── The review view ──────────────────────────────────────────────────

async function loadHtml(id) {
  if (htmlCache[id]) return htmlCache[id];
  try {
    const r = await api("GET", BASE + "/pages/" + id + "/html");
    htmlCache[id] = r.html || "";
  } catch (_) { htmlCache[id] = ""; }
  return htmlCache[id];
}

function pinClass(f) {
  if (f.verdict === "down") return "pin down";
  if (f.verdict === "up" || (f.comment || "").trim()) return "pin done";
  return "pin";
}

// DOMParser gives an inert document — scripts never execute there. Strip
// everything that could run once the nodes go live (defense in depth: the
// submission tool already rejects <script>), then adopt into the shadow
// root. `body {...}` rules can't match inside a shadow tree, so they are
// retargeted at the wrapper.
function sanitizeGeneratedHtml(html) {
  const doc = new DOMParser().parseFromString(html, "text/html");
  doc.querySelectorAll("script").forEach((n) => n.remove());
  doc.querySelectorAll("*").forEach((n) => {
    for (const a of Array.from(n.attributes)) {
      const name = a.name.toLowerCase();
      if (name.startsWith("on")) n.removeAttribute(a.name);
      else if ((name === "href" || name === "src" || name === "xlink:href") &&
               a.value.trim().toLowerCase().startsWith("javascript:")) n.removeAttribute(a.name);
    }
  });
  return doc;
}

function renderDoc(html) {
  const host = document.getElementById("review-host");
  if (!host.shadowRoot) host.attachShadow({ mode: "open" });
  SHADOW = host.shadowRoot;
  const doc = sanitizeGeneratedHtml(html);
  const styles = Array.from(doc.querySelectorAll("style"))
    .map((s) => s.textContent.replace(/(^|[\s,{}])body\b/g, "$1.uig-body")).join("\n");
  const wrap = document.createElement("div");
  wrap.className = "uig-body";
  if (doc.body) {
    const bodyStyle = doc.body.getAttribute("style");
    if (bodyStyle) wrap.setAttribute("style", bodyStyle);
    while (doc.body.firstChild) wrap.appendChild(doc.body.firstChild);
  }
  SHADOW.innerHTML = "";
  const styleEl = document.createElement("style");
  styleEl.textContent = styles;
  SHADOW.appendChild(styleEl);
  SHADOW.appendChild(wrap);
}

function elementRect(id) {
  if (!SHADOW) return null;
  const el = SHADOW.querySelector('[data-uig-id="' + CSS.escape(id) + '"]');
  if (!el) return null;
  const doc = document.getElementById("review-doc").getBoundingClientRect();
  const r = el.getBoundingClientRect();
  return { left: r.left - doc.left, top: r.top - doc.top, width: r.width, height: r.height, el };
}

function renderOverlay() {
  const page = openPage();
  const overlay = document.getElementById("overlay");
  overlay.innerHTML = "";
  if (!page) return;
  (page.elements || []).forEach((e, i) => {
    const r = elementRect(e.id);
    if (!r) return;
    const pin = document.createElement("div");
    pin.className = pinClass(fb(page, e.id));
    pin.textContent = String(i + 1);
    pin.title = e.label;
    pin.style.left = r.left + "px";
    pin.style.top = r.top + "px";
    pin.dataset.el = e.id;
    pin.setAttribute("data-testid", "gauge-pin");
    pin.onclick = () => selectElement(e.id, true);
    overlay.appendChild(pin);
  });
}

function selectElement(id, scrollRail) {
  const page = openPage();
  if (!page) return;
  document.querySelectorAll(".el").forEach((el) => el.classList.toggle("sel", el.dataset.el === id));
  const hl = document.getElementById("hl");
  const r = id === "_page" ? null : elementRect(id);
  if (r) {
    hl.style.display = "";
    hl.style.left = (r.left - 2) + "px";
    hl.style.top = (r.top - 2) + "px";
    hl.style.width = (r.width) + "px";
    hl.style.height = (r.height) + "px";
  } else {
    hl.style.display = "none";
  }
  const item = document.querySelector('.el[data-el="' + CSS.escape(id) + '"]');
  if (item && scrollRail) item.scrollIntoView({ block: "nearest" });
}

function railItem(page, e, idx) {
  const f = fb(page, e.id);
  const suggest = f.verdict === "up" && !f.starred && !f.star_dismissed;
  return '<div class="el" data-el="' + esc(e.id) + '" data-testid="gauge-element">' +
    '<div class="row" style="justify-content:space-between">' +
    '<b>' + (idx != null ? (idx + 1) + ". " : "") + esc(e.label) + '</b>' +
    (e.kind ? '<span class="chip">' + esc(e.kind) + '</span>' : "") +
    '</div>' +
    '<div class="row" style="margin-top:6px">' +
    '<button data-vote="up" data-el-id="' + esc(e.id) + '"' + (f.verdict === "up" ? ' class="active"' : "") + ' data-testid="gauge-verdict-up">&#128077;</button>' +
    '<button data-vote="down" data-el-id="' + esc(e.id) + '"' + (f.verdict === "down" ? ' class="active"' : "") + ' data-testid="gauge-verdict-down">&#128078;</button>' +
    '<button data-star="' + esc(e.id) + '"' + (f.starred ? ' class="active star"' : "") + ' title="Use as visual reference" data-testid="gauge-star">&#9733;' + (f.starred ? " starred" : "") + '</button>' +
    (f.starred && !f.has_shot ? '<span class="chip warn">no screenshot captured</span>' : "") +
    '</div>' +
    (suggest ? '<div class="row" style="margin-top:4px" data-testid="gauge-star-suggest">' +
      '<span class="chip warn">&#9733; suggested — liked elements make good references</span>' +
      '<button data-star="' + esc(e.id) + '">Star</button>' +
      '<button data-dismiss="' + esc(e.id) + '">Dismiss</button></div>' : "") +
    '<textarea placeholder="Why? This comment becomes a prompt line." data-comment="' + esc(e.id) + '" data-testid="gauge-comment">' + esc(f.comment || "") + '</textarea>' +
    '<div class="row" style="margin-top:4px">' +
    '<button class="primary" data-save="' + esc(e.id) + '" data-testid="gauge-save-feedback">Save</button>' +
    '</div></div>';
}

function renderRail() {
  const page = openPage();
  const rail = document.getElementById("rail");
  if (!page) { rail.innerHTML = ""; return; }
  // A vote/star save re-renders the rail when its request settles — which
  // can land mid-typing. Carry unsaved comment drafts across the rebuild
  // so a re-render never eats the user's text.
  const drafts = {};
  rail.querySelectorAll("textarea[data-comment]").forEach((t) => {
    drafts[t.dataset.comment] = t.value;
  });
  const whole = { id: "_page", label: "Whole page", kind: "" };
  rail.innerHTML = railItem(page, whole, null) +
    (page.elements || []).map((e, i) => railItem(page, e, i)).join("");
  rail.querySelectorAll("textarea[data-comment]").forEach((t) => {
    const d = drafts[t.dataset.comment];
    if (d !== undefined && d !== "" && d !== t.value) t.value = d;
  });
}

async function openReview(id) {
  OPEN = id;
  const page = openPage();
  if (!page) return;
  document.getElementById("review-card").style.display = "";
  document.getElementById("review-title").textContent = page.name;
  document.getElementById("review-notes").textContent = page.design_notes || "";
  const html = await loadHtml(id);
  renderDoc(html);
  renderOverlay();
  renderPages();
  renderRail();
  document.getElementById("review-card").scrollIntoView({ block: "start", behavior: "smooth" });
}

function closeReview() {
  OPEN = null;
  document.getElementById("review-card").style.display = "none";
  renderPages();
}

// ── Screenshot capture (starred elements) ────────────────────────────
// The generated page is self-contained (inline CSS, no scripts, no external
// resources — enforced at submission), so the element can be re-rendered
// standalone inside an SVG foreignObject and drawn to a canvas.

function captureElement(pageId, elId) {
  return new Promise((resolve, reject) => {
    const r = elementRect(elId);
    if (!r) return reject(new Error("element not found in the rendered page"));
    const styles = SHADOW
      ? Array.from(SHADOW.querySelectorAll("style")).map((s) => s.textContent).join("\n")
      : "";
    let xml = "";
    try { xml = new XMLSerializer().serializeToString(r.el); }
    catch (e) { return reject(new Error("could not serialize element: " + e.message)); }
    const w = Math.max(1, Math.ceil(r.width));
    const h = Math.max(1, Math.ceil(r.height));
    const wrap = SHADOW && SHADOW.querySelector(".uig-body");
    const bg = wrap ? getComputedStyle(wrap).backgroundColor : "#ffffff";
    const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="' + w + '" height="' + h + '">' +
      '<foreignObject width="100%" height="100%">' +
      '<div xmlns="http://www.w3.org/1999/xhtml" class="uig-body" style="width:' + w + 'px;background:' + bg + '">' +
      '<style>/*<![CDATA[*/' + styles + '/*]]>*/</style>' + xml +
      '</div></foreignObject></svg>';
    // A data: URL, not a blob URL — this page runs with an opaque origin
    // (sandboxed frame), where blob loads are unreliable; data: SVGs load
    // anywhere and keep the canvas clean.
    const url = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(svg);
    const img = new Image();
    img.onload = () => {
      // Downscale wide elements so the base64 stays under the store cap.
      const scale = Math.min(1, 1200 / w);
      const canvas = document.createElement("canvas");
      canvas.width = Math.max(1, Math.round(w * scale));
      canvas.height = Math.max(1, Math.round(h * scale));
      const ctx = canvas.getContext("2d");
      ctx.fillStyle = "#ffffff";
      ctx.fillRect(0, 0, canvas.width, canvas.height);
      ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
      try {
        for (const q of [0.85, 0.65, 0.45, 0.25]) {
          const data = canvas.toDataURL("image/jpeg", q);
          const b64 = data.slice(data.indexOf(",") + 1);
          if (b64.length <= 190000) {
            return api("POST", BASE + "/shots", {
              page_id: pageId, element_id: elId, image_base64: b64, mime_type: "image/jpeg",
            }).then(resolve, reject);
          }
        }
        reject(new Error("screenshot too large even after downscaling"));
      } catch (e) { reject(new Error("capture failed: " + e.message)); }
    };
    img.onerror = () => reject(new Error("could not rasterize element"));
    img.src = url;
  });
}

// ── Actions ──────────────────────────────────────────────────────────

async function saveFeedback(patch) {
  const r = await api("POST", BASE + "/feedback", patch);
  // Patch local state so open comment drafts elsewhere survive.
  const page = STATE.pages.find((p) => p.id === patch.page_id);
  if (page && r.feedback) {
    page.feedback = page.feedback || {};
    const f = r.feedback;
    page.feedback[patch.element_id] = {
      verdict: f.verdict, comment: f.comment, starred: f.starred,
      star_dismissed: f.star_dismissed,
      has_shot: (page.feedback[patch.element_id] || {}).has_shot || false,
    };
  }
  // The composed prompt changed — refetch it (cheap) without re-rendering the rail.
  try {
    const s = await api("GET", BASE + "/state");
    STATE.prompt = s.prompt; STATE.folders = s.folders;
    renderPrompt();
  } catch (_) {}
}

document.getElementById("g-btn").onclick = async () => {
  const folder_id = document.getElementById("g-folder").value;
  const model = document.getElementById("g-model").value;
  const brief = document.getElementById("g-brief").value;
  if (!folder_id) { banner("pick a folder for the generation session"); return; }
  if (!model) { banner("pick a model"); return; }
  try { banner(""); await api("POST", BASE + "/generate", { folder_id, model, brief }); await refresh(); }
  catch (e) { banner(e.message); }
};

document.getElementById("pages").addEventListener("click", async (ev) => {
  const open = ev.target.closest("button[data-open]");
  const del = ev.target.closest("button[data-del]");
  try {
    banner("");
    if (open) await openReview(open.dataset.open);
    else if (del) {
      if (!confirm("Delete this page? Its feedback and starred references leave the prompt too.")) return;
      await api("POST", BASE + "/pages/" + del.dataset.del + "/delete");
      delete htmlCache[del.dataset.del];
      if (OPEN === del.dataset.del) closeReview();
      await refresh();
    }
  } catch (e) { banner(e.message); }
});

document.getElementById("review-close").onclick = closeReview;

document.getElementById("rail").addEventListener("click", async (ev) => {
  const page = openPage();
  if (!page) return;
  const vote = ev.target.closest("button[data-vote]");
  const star = ev.target.closest("button[data-star]");
  const dismiss = ev.target.closest("button[data-dismiss]");
  const save = ev.target.closest("button[data-save]");
  const item = ev.target.closest(".el");
  try {
    banner("");
    if (vote) {
      const id = vote.dataset.elId;
      const cur = fb(page, id).verdict;
      const next = cur === vote.dataset.vote ? "" : vote.dataset.vote;
      await saveFeedback({ page_id: page.id, element_id: id, verdict: next });
      renderRail(); renderOverlay(); selectElement(id, false);
    } else if (star) {
      const id = star.dataset.star;
      const starred = !fb(page, id).starred;
      if (starred && id !== "_page") {
        try {
          await captureElement(page.id, id);
          page.feedback = page.feedback || {};
          (page.feedback[id] = page.feedback[id] || {}).has_shot = true;
        } catch (e) { banner("starred without screenshot: " + e.message); }
      }
      await saveFeedback({ page_id: page.id, element_id: id, starred });
      renderRail(); renderOverlay();
    } else if (dismiss) {
      await saveFeedback({ page_id: page.id, element_id: dismiss.dataset.dismiss, star_dismissed: true });
      renderRail();
    } else if (save) {
      const id = save.dataset.save;
      const ta = document.querySelector('textarea[data-comment="' + CSS.escape(id) + '"]');
      await saveFeedback({ page_id: page.id, element_id: id, comment: ta ? ta.value : "" });
      renderRail(); renderOverlay(); renderPages();
    } else if (item) {
      selectElement(item.dataset.el, false);
    }
  } catch (e) { banner(e.message); }
});

document.getElementById("folders").addEventListener("change", async (ev) => {
  const box = ev.target.closest("input[data-folder]");
  if (!box) return;
  try {
    banner("");
    await api("POST", BASE + "/folders", { folder_id: box.dataset.folder, enabled: box.checked });
    const f = STATE.folders.find((x) => x.id === box.dataset.folder);
    if (f) f.enabled = box.checked;
    renderPrompt();
  } catch (e) { banner(e.message); }
});

document.getElementById("copy-prompt").onclick = () => {
  const text = STATE.prompt.text || "";
  const done = () => {
    const chip = document.getElementById("copy-done");
    chip.style.display = "";
    setTimeout(() => { chip.style.display = "none"; }, 1500);
  };
  if (navigator.clipboard && navigator.clipboard.writeText) {
    navigator.clipboard.writeText(text).then(done, () => fallbackCopy(text, done));
  } else fallbackCopy(text, done);
};
function fallbackCopy(text, done) {
  const ta = document.createElement("textarea");
  ta.value = text;
  document.body.appendChild(ta);
  ta.select();
  try { document.execCommand("copy"); done(); } catch (_) {}
  ta.remove();
}

function render() { renderGenerate(); renderPrompt(); renderPages(); if (OPEN) renderRail(); }

async function refresh() {
  try {
    const keep = OPEN;
    STATE = await api("GET", BASE + "/state");
    if (keep && !STATE.pages.some((p) => p.id === keep)) closeReview();
    document.getElementById("stale").style.display = "none";
    render();
    if (OPEN) renderOverlay();
  }
  catch (e) { banner(e.message); }
}
document.getElementById("refresh-btn").onclick = refresh;
// NO automatic refresh, ever: a re-render wipes in-progress comment
// drafts, so the page must never reload state except on explicit user
// action. The WebSocket below (ticket-authed via the parent frame; the
// JWT never enters this sandbox) only lights the "data changed" chip —
// the user presses Refresh when ready. "locks" is the cross-instance
// lease collection: written per run, never rendered, so it stays ignored.
function markStale() {
  document.getElementById("stale").style.display = "";
}
function requestTicket() {
  return new Promise((resolve) => {
    const onMsg = (e) => {
      const m = e.data;
      if (!m || m.type !== "plugin-ui-ws-ticket-result") return;
      window.removeEventListener("message", onMsg);
      resolve(m.ticket || null);
    };
    window.addEventListener("message", onMsg);
    parent.postMessage({ type: "plugin-ui-ws-ticket" }, "*");
    setTimeout(() => { window.removeEventListener("message", onMsg); resolve(null); }, 10000);
  });
}
let wsBackoff = 1000;
let wsHadConnection = false;
async function connectEvents() {
  const ticket = await requestTicket();
  if (!ticket) {
    setTimeout(connectEvents, wsBackoff);
    wsBackoff = Math.min(wsBackoff * 2, 30000);
    return;
  }
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  const ws = new WebSocket(proto + "//" + location.host + "/ws/plugin-ui?ticket=" + encodeURIComponent(ticket));
  ws.onopen = () => {
    wsBackoff = 1000;
    // A reconnect may have missed change events — flag, never reload.
    if (wsHadConnection) markStale();
    wsHadConnection = true;
  };
  ws.onmessage = (ev) => {
    let m = null;
    try { m = JSON.parse(ev.data); } catch (_) {}
    if (m && m.collection === "locks") return;
    markStale();
  };
  ws.onclose = () => {
    setTimeout(connectEvents, wsBackoff);
    wsBackoff = Math.min(wsBackoff * 2, 30000);
  };
}
async function loadPickers() {
  PICKERS = await api("GET", BASE + "/pickers");
}
async function boot() {
  try { await loadPickers(); }
  catch (e) { banner("Failed to load folder/model lists: " + e.message); }
  await refresh();
  connectEvents();
  // Retry while empty so a slow or failed first fetch never leaves the
  // generation dropdowns permanently blank. Only rewrites the two
  // dropdowns (never in-progress feedback) and stops once populated.
  const pickerTimer = setInterval(async () => {
    if (PICKERS.folders.length && PICKERS.models.length) { clearInterval(pickerTimer); return; }
    try { await loadPickers(); renderGenerate(); } catch (_) {}
  }, 5000);
}
boot();
</script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_html_has_bridge_review_and_testids() {
        assert!(PAGE_HTML.contains("plugin-ui-fetch"));
        // Change notifications ride the page's own ticket-authed WebSocket,
        // but they must NEVER auto-refresh the page — a re-render wipes
        // in-progress comment drafts. Events only light the stale chip; the
        // user reloads via the Refresh button.
        assert!(PAGE_HTML.contains("plugin-ui-ws-ticket"));
        assert!(PAGE_HTML.contains("/ws/plugin-ui?ticket="));
        assert!(!PAGE_HTML.contains("setInterval(refresh"));
        assert!(!PAGE_HTML.contains("scheduleRefresh"));
        for id in [
            "gauge-refresh",
            "gauge-stale",
            "gauge-generate",
            "gauge-gen-status",
            "gauge-gen-brief",
            "gauge-page",
            "gauge-open-review",
            "gauge-review",
            "gauge-element",
            "gauge-verdict-up",
            "gauge-verdict-down",
            "gauge-comment",
            "gauge-save-feedback",
            "gauge-star",
            "gauge-prompt",
            "gauge-ingredients",
            "gauge-folder-toggle",
            "gauge-copy-prompt",
        ] {
            assert!(
                PAGE_HTML.contains(&format!("data-testid=\"{id}\"")),
                "missing testid {id}"
            );
        }
        // Pins are DOM-created; their testid rides setAttribute.
        assert!(PAGE_HTML.contains("setAttribute(\"data-testid\", \"gauge-pin\")"));
        // Generated markup renders in a sanitized shadow root — parsed
        // inert, scripts and on*/javascript: stripped — never in an iframe
        // (the sandboxed plugin frame makes any nested frame cross-origin).
        assert!(PAGE_HTML.contains("sanitizeGeneratedHtml"));
        assert!(PAGE_HTML.contains("DOMParser"));
        assert!(PAGE_HTML.contains("attachShadow"));
        assert!(!PAGE_HTML.contains("review-frame"));
        assert!(!PAGE_HTML.contains("srcdoc"));
        assert!(PAGE_HTML.contains("/api/plugin-ui/ui-gauge"));
    }
}
