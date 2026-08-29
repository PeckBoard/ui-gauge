//! The UI Gauge page: generate-a-baseline loop, baseline gallery with 1-10
//! ranking sliders, the category/bar editor, the living overall baseline
//! prompt, and the evaluation history. Served like the other plugin pages:
//! public HTML shell on `GET /plugin-api/v1/ui-gauge`, authed JSON under
//! `/api/plugin-ui/ui-gauge/*`, reached through the parent `plugin-ui-fetch`
//! postMessage bridge. Uploaded images are downscaled client-side; generated
//! baselines are agent-submitted HTML rendered in a script-less sandboxed
//! frame.

use serde_json::{Value, json};

use crate::gauge::{
    self, BASELINE_HTML_COLLECTION, BASELINE_IMAGES_COLLECTION, BASELINES_COLLECTION, Baseline,
    Category,
};
use crate::host::{HostFn, call_host};

pub const PAGE_PATH: &str = "/plugin-api/v1/ui-gauge";
const API_PREFIX: &str = "/api/plugin-ui/ui-gauge";

/// Keep one image comfortably under the 256 KB store-document cap
/// (base64 of 150 KB ≈ 200 KB, plus JSON overhead).
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

    let out = match (method.as_str(), segs.as_slice()) {
        ("GET", ["state"]) => state_route(),
        ("GET", ["pickers"]) => pickers_route(),
        ("POST", ["categories"]) => categories_route(&body),
        ("POST", ["generate"]) => generate_route(&body),
        ("POST", ["baselines"]) => create_baseline_route(&body),
        ("POST", ["baselines", id]) => update_baseline_route(id, &body),
        ("POST", ["baselines", id, "delete"]) => {
            gauge::store_delete(BASELINES_COLLECTION, id);
            gauge::store_delete(BASELINE_IMAGES_COLLECTION, id);
            gauge::store_delete(BASELINE_HTML_COLLECTION, id);
            Ok(json_response(200, json!({ "ok": true })))
        }
        ("GET", ["baselines", id, "image"]) => {
            match gauge::store_get(BASELINE_IMAGES_COLLECTION, id)? {
                Some(img) => Ok(json_response(200, img)),
                None => Ok(json_response(404, json!({ "error": "no image" }))),
            }
        }
        ("GET", ["baselines", id, "html"]) => {
            match gauge::store_get(BASELINE_HTML_COLLECTION, id)? {
                Some(html) => Ok(json_response(200, html)),
                None => Ok(json_response(404, json!({ "error": "no html" }))),
            }
        }
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
    let cats = gauge::categories();
    let baselines = gauge::baselines();
    let categories: Vec<Value> = cats
        .iter()
        .map(|c| {
            json!({
                "key": c.key,
                "label": c.label,
                "bar_override": c.bar_override,
                "bar": gauge::bar_for(c, &baselines),
            })
        })
        .collect();
    let overall = gauge::overall_prompt(&baselines);
    let baseline_views: Vec<Value> = baselines
        .iter()
        .map(|b| {
            let mut v = serde_json::to_value(b).unwrap_or(Value::Null);
            if let Some(map) = v.as_object_mut() {
                map.insert("avg".into(), json!(b.avg_score()));
            }
            v
        })
        .collect();
    let mut evals = gauge::evaluations(None);
    evals.truncate(50);
    Ok(json_response(
        200,
        json!({
            "categories": categories,
            "baselines": baseline_views,
            "evaluations": evals,
            "overall_prompt": overall,
            "generation": gauge::generation_state(),
        }),
    ))
}

fn categories_route(body: &Value) -> Result<Value, String> {
    let cats: Vec<Category> = serde_json::from_value(
        body.get("categories")
            .cloned()
            .ok_or("'categories' required")?,
    )
    .map_err(|e| format!("bad categories: {e}"))?;
    if cats.is_empty() {
        return Err("at least one category is required".into());
    }
    for c in &cats {
        if c.key.trim().is_empty() || c.label.trim().is_empty() {
            return Err("category key and label must not be blank".into());
        }
        if !c
            .key
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
        {
            return Err(format!("category key '{}' must be lower_snake_case", c.key));
        }
    }
    gauge::save_categories(&cats)?;
    Ok(json_response(200, json!({ "ok": true })))
}

/// The button: spawn a temp generation session in the chosen folder with the
/// chosen model, hand it the iteration prompt, and record the pending
/// generation. The session submits back via `ui_gauge_submit_baseline`.
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
        .ok_or("'model' is required — pick the model that generates the baseline")?;

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

    let baselines = gauge::baselines();
    let iteration = baselines.iter().filter(|b| b.kind == "generated").count() + 1;
    let created = call_host(
        HostFn::CreateSession,
        &json!({
            "name": format!("UI Gauge baseline #{iteration}"),
            "model": model,
            "is_temp": true,
            "folder_path": folder_path,
            "system_prompt": "You generate one baseline UI iteration for the ui-gauge plugin \
        and submit it via the ui_gauge_submit_baseline MCP tool. Follow the task prompt exactly; \
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
            "prompt": gauge::build_generation_prompt(&baselines),
        }),
    )?;
    gauge::set_generation_state(json!({
        "status": "running",
        "session_id": session_id,
        "model": model,
        "started_at": gauge::clock(),
    }));
    Ok(json_response(
        200,
        json!({ "ok": true, "session_id": session_id }),
    ))
}

fn parse_scores(v: Option<&Value>) -> std::collections::BTreeMap<String, u8> {
    v.and_then(|s| s.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| {
                    v.as_u64()
                        .filter(|n| (1..=10).contains(n))
                        .map(|n| (k.clone(), n as u8))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn create_baseline_route(body: &Value) -> Result<Value, String> {
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("'name' is required")?
        .to_string();
    let image_base64 = body
        .get("image_base64")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("'image_base64' is required")?;
    if image_base64.len() > MAX_IMAGE_BASE64_LEN {
        return Err(format!(
            "image too large ({} chars base64, max {MAX_IMAGE_BASE64_LEN}) — the page \
             should have downscaled it; try a smaller screenshot",
            image_base64.len()
        ));
    }
    let mime_type = body
        .get("mime_type")
        .and_then(|v| v.as_str())
        .unwrap_or("image/jpeg")
        .to_string();
    let id = gauge::new_id("base");
    let baseline = Baseline {
        id: id.clone(),
        name,
        notes: body
            .get("notes")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        scores: parse_scores(body.get("scores")),
        mime_type: mime_type.clone(),
        created_at: gauge::clock(),
        kind: "image".into(),
        change_prompt: String::new(),
    };
    gauge::store_put(
        BASELINES_COLLECTION,
        &id,
        serde_json::to_value(&baseline).map_err(|e| e.to_string())?,
    )?;
    gauge::store_put(
        BASELINE_IMAGES_COLLECTION,
        &id,
        json!({ "image_base64": image_base64, "mime_type": mime_type }),
    )?;
    Ok(json_response(200, json!({ "ok": true, "id": id })))
}

/// Rating writes: the overall prompt recomputes on every read, so saving new
/// scores here is all "keeping it up to date" requires.
fn update_baseline_route(id: &str, body: &Value) -> Result<Value, String> {
    let mut b: Baseline = gauge::store_get(BASELINES_COLLECTION, id)?
        .and_then(|v| serde_json::from_value(v).ok())
        .ok_or_else(|| format!("baseline not found: {id}"))?;
    if let Some(name) = body.get("name").and_then(|v| v.as_str()) {
        if name.trim().is_empty() {
            return Err("name must not be blank".into());
        }
        b.name = name.trim().to_string();
    }
    if let Some(notes) = body.get("notes").and_then(|v| v.as_str()) {
        b.notes = notes.to_string();
    }
    if body.get("scores").is_some() {
        b.scores = parse_scores(body.get("scores"));
    }
    gauge::store_put(
        BASELINES_COLLECTION,
        id,
        serde_json::to_value(&b).map_err(|e| e.to_string())?,
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
  button:disabled { opacity: .5; cursor: default; }
  input[type=text], textarea, select { width: 100%; padding: 6px 8px; border: 1px solid var(--line);
    border-radius: 8px; background: var(--bg); color: var(--text); font: inherit; }
  input[type=number] { width: 64px; padding: 4px 6px; border: 1px solid var(--line);
    border-radius: 6px; background: var(--bg); color: var(--text); font: inherit; }
  table { width: 100%; border-collapse: collapse; font-size: 13px; }
  th, td { text-align: left; padding: 5px 8px; border-bottom: 1px solid var(--line); vertical-align: top; }
  th { color: var(--muted); font-weight: 500; }
  .muted { color: var(--muted); font-size: 12px; }
  .error-banner { color: var(--bad); margin: 8px 0; }
  .gallery { display: grid; grid-template-columns: repeat(auto-fill, minmax(300px, 1fr)); gap: 12px; }
  .baseline img, .baseline iframe { width: 100%; height: 210px; border-radius: 8px;
    border: 1px solid var(--line); background: #fff; object-fit: cover; }
  .baseline iframe { pointer-events: none; }
  .slider-row { display: flex; align-items: center; gap: 8px; margin: 3px 0; }
  .slider-row label { flex: 1; font-size: 12px; color: var(--muted); }
  .slider-row input[type=range] { flex: 2; }
  .slider-row output { width: 20px; text-align: right; font-weight: 600; }
  .verdict-pass { color: var(--ok); font-weight: 600; }
  .verdict-subpar { color: var(--bad); font-weight: 600; }
  .row { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
  .chip { display: inline-block; padding: 1px 9px; border-radius: 999px; font-size: 12px;
          border: 1px solid var(--line); color: var(--muted); }
  .chip.ok { color: var(--ok); border-color: var(--ok); }
  .chip.warn { color: var(--warn); border-color: var(--warn); }
  pre.prompt { white-space: pre-wrap; background: var(--bg); border: 1px solid var(--line);
    border-radius: 8px; padding: 10px; font-size: 12px; max-height: 280px; overflow: auto; }
  .change { font-size: 12px; border-left: 3px solid var(--accent); padding: 4px 8px;
    margin: 6px 0; color: var(--muted); }
</style>
</head>
<body>
<h1>UI Gauge</h1>
<div id="banner" class="error-banner" style="display:none"></div>

<div class="card">
  <h2>Generate a baseline</h2>
  <div class="muted">One button press spawns a temp agent session that designs the next baseline UI —
  it applies every directive you have already validated, changes one aspect, and explains the change.
  Rate the result below: 7+ average folds the change into the overall prompt; 4− marks it as one to avoid.</div>
  <div class="row" style="margin-top:8px">
    <select id="g-folder" style="flex:1" data-testid="gauge-gen-folder"></select>
    <select id="g-model" style="flex:1" data-testid="gauge-gen-model"></select>
    <button class="primary" id="g-btn" data-testid="gauge-generate">Generate baseline</button>
  </div>
  <div id="g-status" class="muted" style="margin-top:6px" data-testid="gauge-gen-status"></div>
</div>

<div class="card">
  <h2>Overall baseline prompt</h2>
  <div class="muted">Your validated style directives — rebuilt automatically from every generated
  baseline you rated 7+/10 (re-rating updates it instantly). Agents get it from ui_gauge_rubric.</div>
  <pre class="prompt" id="overall" data-testid="gauge-overall"></pre>
</div>

<div class="card">
  <h2>Categories &amp; bars</h2>
  <div class="muted">The bar per category defaults to the median of your baseline rankings; set an override to pin it. Work scoring below a bar is subpar and creates follow-up cards.</div>
  <table id="cats"></table>
  <div class="row" style="margin-top:8px">
    <button id="cat-add">Add category</button>
    <button class="primary" id="cat-save" data-testid="gauge-save-cats">Save categories</button>
  </div>
</div>

<div class="card">
  <h2>Add a screenshot baseline</h2>
  <div class="muted">Upload a reference screenshot and rank it 1-10 per category. These rankings calibrate the agent's scoring to YOUR scale. Images are downscaled to fit storage.</div>
  <div class="row" style="margin:8px 0">
    <input type="file" id="b-file" accept="image/*" data-testid="gauge-file">
    <input type="text" id="b-name" placeholder="Name, e.g. 'Settings page — good'" style="flex:1" data-testid="gauge-base-name">
  </div>
  <input type="text" id="b-notes" placeholder="Notes (optional): what makes this reference rank where it does">
  <div id="b-sliders" style="margin-top:8px"></div>
  <button class="primary" id="b-save" style="margin-top:8px" data-testid="gauge-base-save">Add baseline</button>
</div>

<div class="card">
  <h2>Baselines</h2>
  <div id="gallery" class="gallery"></div>
</div>

<div class="card">
  <h2>Evaluation history</h2>
  <table id="evals"></table>
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

let STATE = { categories: [], baselines: [], evaluations: [], overall_prompt: "", generation: { status: "idle" } };
let PICKERS = { folders: [], models: [] };
const imgCache = {};
const htmlCache = {};

function banner(msg) {
  const el = document.getElementById("banner");
  el.style.display = msg ? "" : "none";
  el.textContent = msg || "";
}

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
  if (g.status === "running") st.textContent = "Generating… the agent session is designing the next baseline (this page updates itself).";
  else if (g.status === "done") st.textContent = "Last generation submitted a baseline — rate it below.";
  else if (g.status === "ended") st.textContent = "The last generation session ended WITHOUT submitting a baseline — try again (a stronger model helps).";
  else st.textContent = "";
}

function renderOverall() {
  document.getElementById("overall").textContent = STATE.overall_prompt || "";
}

function renderCats() {
  const t = document.getElementById("cats");
  t.innerHTML = "<tr><th>Key (lower_snake_case)</th><th>Label</th><th>Bar override</th><th>Effective bar</th><th></th></tr>" +
    STATE.categories.map((c, i) =>
      '<tr>' +
      '<td><input type="text" data-cat-key="' + i + '" value="' + esc(c.key) + '"></td>' +
      '<td><input type="text" data-cat-label="' + i + '" value="' + esc(c.label) + '"></td>' +
      '<td><input type="number" min="1" max="10" data-cat-bar="' + i + '" value="' + (c.bar_override == null ? "" : c.bar_override) + '" placeholder="auto"></td>' +
      '<td><b>' + c.bar + '</b></td>' +
      '<td><button class="danger" data-cat-del="' + i + '">remove</button></td>' +
      '</tr>').join("");
}
function sliderRows(prefix, scores) {
  return STATE.categories.map((c) =>
    '<div class="slider-row"><label>' + esc(c.label) + '</label>' +
    '<input type="range" min="1" max="10" value="' + ((scores || {})[c.key] || 5) + '" data-' + prefix + '="' + esc(c.key) + '" ' +
    'oninput="this.nextElementSibling.value=this.value">' +
    '<output>' + ((scores || {})[c.key] || 5) + '</output></div>').join("");
}
function renderNewBaselineSliders() {
  document.getElementById("b-sliders").innerHTML = sliderRows("newscore", {});
}
async function loadImage(id) {
  if (imgCache[id]) return imgCache[id];
  try {
    const r = await api("GET", BASE + "/baselines/" + id + "/image");
    imgCache[id] = "data:" + (r.mime_type || "image/jpeg") + ";base64," + r.image_base64;
  } catch (_) { imgCache[id] = ""; }
  return imgCache[id];
}
async function loadHtml(id) {
  if (htmlCache[id]) return htmlCache[id];
  try {
    const r = await api("GET", BASE + "/baselines/" + id + "/html");
    htmlCache[id] = r.html || "";
  } catch (_) { htmlCache[id] = ""; }
  return htmlCache[id];
}
function renderGallery() {
  const g = document.getElementById("gallery");
  if (!STATE.baselines.length) {
    g.innerHTML = '<div class="muted">No baselines yet — press Generate baseline, or upload reference screenshots.</div>';
    return;
  }
  g.innerHTML = STATE.baselines.slice().reverse().map((b) => {
    const rated = b.avg != null;
    const chip = !rated ? '<span class="chip warn">unrated</span>' :
      '<span class="chip' + (b.avg >= 7 ? ' ok' : '') + '">avg ' + b.avg.toFixed(1) + (b.avg >= 7 && b.kind === 'generated' ? ' → in overall prompt' : '') + '</span>';
    const preview = b.kind === 'generated'
      ? '<iframe sandbox="" data-html="' + esc(b.id) + '" title="' + esc(b.name) + '"></iframe>'
      : '<img data-img="' + esc(b.id) + '" alt="' + esc(b.name) + '">';
    return '<div class="baseline card" data-testid="gauge-baseline" data-base="' + esc(b.id) + '">' +
      preview +
      '<div class="row"><b>' + esc(b.name) + '</b>' + chip + '</div>' +
      (b.kind === 'generated' && b.change_prompt ?
        '<div class="change" data-testid="gauge-change">' + esc(b.change_prompt) + '</div>' : "") +
      (b.notes ? '<div class="muted">' + esc(b.notes) + '</div>' : "") +
      sliderRows("score-" + b.id, b.scores) +
      '<div class="row" style="margin-top:6px">' +
      '<button class="primary" data-base-save="' + esc(b.id) + '">Save rankings</button>' +
      '<button class="danger" data-base-del="' + esc(b.id) + '">Delete</button>' +
      '</div></div>';
  }).join("");
  STATE.baselines.forEach(async (b) => {
    if (b.kind === 'generated') {
      const el = g.querySelector('iframe[data-html="' + b.id + '"]');
      if (el) el.srcdoc = await loadHtml(b.id);
    } else {
      const el = g.querySelector('img[data-img="' + b.id + '"]');
      if (el) el.src = await loadImage(b.id);
    }
  });
}
function renderEvals() {
  const t = document.getElementById("evals");
  if (!STATE.evaluations.length) {
    t.innerHTML = '<tr><td class="muted">No evaluations yet — agents submit them via ui_gauge_score.</td></tr>';
    return;
  }
  t.innerHTML = "<tr><th>When</th><th>Target</th><th>Verdict</th><th>Scores</th><th>Gaps / cards</th></tr>" +
    STATE.evaluations.map((e) => {
      const scores = Object.entries(e.scores || {}).map(([k, v]) => k + ":" + v).join(" ");
      const gaps = (e.gaps || []).map((g) => g.label + " " + g.score + "<" + g.bar).join(", ");
      const cards = (e.cards_created || []).length;
      return '<tr data-testid="gauge-eval">' +
        '<td>' + esc((e.ts || "").replace("T", " ").slice(0, 16)) + '</td>' +
        '<td>' + esc(e.target) + '</td>' +
        '<td class="verdict-' + esc(e.verdict) + '">' + esc(e.verdict) + '</td>' +
        '<td class="muted">' + esc(scores) + '</td>' +
        '<td>' + esc(gaps || "—") + (cards ? ' <span class="muted">(' + cards + ' card(s))</span>' : "") + '</td>' +
        '</tr>';
    }).join("");
}
function render() { renderGenerate(); renderOverall(); renderCats(); renderNewBaselineSliders(); renderGallery(); renderEvals(); }

document.getElementById("g-btn").onclick = async () => {
  const folder_id = document.getElementById("g-folder").value;
  const model = document.getElementById("g-model").value;
  if (!folder_id) { banner("pick a folder for the generation session"); return; }
  if (!model) { banner("pick a model"); return; }
  try { banner(""); await api("POST", BASE + "/generate", { folder_id, model }); await refresh(); }
  catch (e) { banner(e.message); }
};

document.getElementById("cats").addEventListener("click", (ev) => {
  const del = ev.target.closest("button[data-cat-del]");
  if (del) { STATE.categories.splice(Number(del.dataset.catDel), 1); renderCats(); }
});
document.getElementById("cat-add").onclick = () => {
  STATE.categories.push({ key: "new_category", label: "New category", bar_override: null, bar: 7 });
  renderCats();
};
document.getElementById("cat-save").onclick = async () => {
  const cats = STATE.categories.map((c, i) => ({
    key: document.querySelector('[data-cat-key="' + i + '"]').value.trim(),
    label: document.querySelector('[data-cat-label="' + i + '"]').value.trim(),
    bar_override: (() => {
      const v = document.querySelector('[data-cat-bar="' + i + '"]').value;
      return v === "" ? null : Number(v);
    })(),
  }));
  try { banner(""); await api("POST", BASE + "/categories", { categories: cats }); await refresh(); }
  catch (e) { banner(e.message); }
};

// Downscale to ≤1200px wide JPEG so the base64 stays under the store cap.
function fileToBase64(file) {
  return new Promise((resolve, reject) => {
    const img = new Image();
    const url = URL.createObjectURL(file);
    img.onload = () => {
      URL.revokeObjectURL(url);
      const scale = Math.min(1, 1200 / img.width);
      const canvas = document.createElement("canvas");
      canvas.width = Math.round(img.width * scale);
      canvas.height = Math.round(img.height * scale);
      canvas.getContext("2d").drawImage(img, 0, 0, canvas.width, canvas.height);
      for (const q of [0.8, 0.6, 0.4, 0.25]) {
        const data = canvas.toDataURL("image/jpeg", q);
        const b64 = data.slice(data.indexOf(",") + 1);
        if (b64.length <= 190000) return resolve(b64);
      }
      reject(new Error("screenshot too large even after downscaling"));
    };
    img.onerror = () => { URL.revokeObjectURL(url); reject(new Error("not an image")); };
    img.src = url;
  });
}
document.getElementById("b-save").onclick = async () => {
  const file = document.getElementById("b-file").files[0];
  if (!file) { banner("choose a screenshot first"); return; }
  const scores = {};
  document.querySelectorAll("#b-sliders input[type=range]").forEach((el) => {
    scores[el.dataset.newscore] = Number(el.value);
  });
  try {
    banner("");
    const image_base64 = await fileToBase64(file);
    await api("POST", BASE + "/baselines", {
      name: document.getElementById("b-name").value,
      notes: document.getElementById("b-notes").value,
      scores, image_base64, mime_type: "image/jpeg",
    });
    document.getElementById("b-file").value = "";
    document.getElementById("b-name").value = "";
    document.getElementById("b-notes").value = "";
    await refresh();
  } catch (e) { banner(e.message); }
};
document.getElementById("gallery").addEventListener("click", async (ev) => {
  const save = ev.target.closest("button[data-base-save]");
  const del = ev.target.closest("button[data-base-del]");
  try {
    banner("");
    if (save) {
      const id = save.dataset.baseSave;
      const scores = {};
      document.querySelectorAll('input[data-score-' + CSS.escape(id) + ']').forEach((el) => {
        scores[el.getAttribute("data-score-" + id)] = Number(el.value);
      });
      await api("POST", BASE + "/baselines/" + id, { scores });
      await refresh();
    } else if (del) {
      const id = del.dataset.baseDel;
      if (!confirm("Delete this baseline? A deleted high-rated baseline also leaves the overall prompt.")) return;
      await api("POST", BASE + "/baselines/" + id + "/delete");
      delete imgCache[id]; delete htmlCache[id];
      await refresh();
    }
  } catch (e) { banner(e.message); }
});

async function refresh() {
  try { STATE = await api("GET", BASE + "/state"); render(); }
  catch (e) { banner(e.message); }
}
async function loadPickers() {
  PICKERS = await api("GET", BASE + "/pickers");
}
async function boot() {
  try { await loadPickers(); }
  catch (e) { banner("Failed to load folder/model lists: " + e.message); }
  await refresh();
  setInterval(refresh, 5000);
  // Retry while empty so a slow or failed first fetch never leaves the
  // generation dropdowns permanently blank.
  setInterval(async () => {
    if (!PICKERS.folders.length || !PICKERS.models.length) {
      try { await loadPickers(); renderGenerate(); } catch (_) {}
    }
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
    fn page_html_has_bridge_generation_and_testids() {
        assert!(PAGE_HTML.contains("plugin-ui-fetch"));
        assert!(PAGE_HTML.contains("data-testid=\"gauge-baseline\""));
        assert!(PAGE_HTML.contains("data-testid=\"gauge-eval\""));
        assert!(PAGE_HTML.contains("data-testid=\"gauge-generate\""));
        assert!(PAGE_HTML.contains("data-testid=\"gauge-overall\""));
        assert!(PAGE_HTML.contains("data-testid=\"gauge-gen-status\""));
        // Generated previews must never execute scripts.
        assert!(PAGE_HTML.contains("iframe sandbox=\"\""));
        assert!(PAGE_HTML.contains("/api/plugin-ui/ui-gauge"));
    }

    #[test]
    fn parse_scores_filters_out_of_range() {
        let s = parse_scores(Some(&json!({ "a": 5, "b": 0, "c": 11, "d": "x" })));
        assert_eq!(s.len(), 1);
        assert_eq!(s.get("a"), Some(&5u8));
    }
}
