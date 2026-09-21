//! UI-gauge domain logic (0.3.0): generated pages with marked elements,
//! per-element user feedback, the per-folder UI preference prompt composed
//! from that feedback, and starred-element screenshots agents can fetch as
//! visual references.
//!
//! The plugin never looks at pixels — generation happens in a temp agent
//! session, screenshots are captured client-side on the UI Gauge page, and
//! the composed preference prompt is what steers future UI work.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::host::{HostFn, call_host};

/// Generated pages' metadata (element manifest included), keyed by page id.
pub const PAGES_COLLECTION: &str = "pages";
/// Generated pages' full HTML documents, keyed by page id — kept out of the
/// page record so listing pages never drags the documents along.
pub const PAGE_HTML_COLLECTION: &str = "page_html";
/// Per-element feedback, keyed `"<page_id>:<element_id>"`.
pub const FEEDBACK_COLLECTION: &str = "feedback";
/// Starred-element screenshots (base64), keyed `"<page_id>:<element_id>"`.
pub const SHOTS_COLLECTION: &str = "shots";
/// Per-folder prompt toggle, keyed by folder id.
pub const FOLDER_PREFS_COLLECTION: &str = "folder_prefs";
/// session id → hash of the preference block last written to that session.
pub const SESSION_PROMPT_COLLECTION: &str = "session_prompt";
pub const ENGINE_COLLECTION: &str = "engine";

/// Generated HTML cap — stays under the 256 KB store-document ceiling.
pub const HTML_MAX_LEN: usize = 180_000;
pub const MAX_ELEMENTS: usize = 40;
pub const PAGES_CAP: usize = 40;
/// Element id for page-level (whole page) feedback.
pub const PAGE_ELEMENT_ID: &str = "_page";

// ── Model ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageElement {
    pub id: String,
    pub label: String,
    /// Optional coarse kind ("header", "card", "form", …) — display only.
    #[serde(default)]
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub id: String,
    pub name: String,
    /// The user's brief this page was generated from ("" = agent's pick).
    #[serde(default)]
    pub brief: String,
    #[serde(default)]
    pub model: String,
    /// The generating agent's notes on what it tried.
    #[serde(default)]
    pub design_notes: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub elements: Vec<PageElement>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Feedback {
    pub page_id: String,
    pub element_id: String,
    /// "up", "down", or "" (no verdict yet).
    #[serde(default)]
    pub verdict: String,
    #[serde(default)]
    pub comment: String,
    /// Starred = use as a visual reference (screenshot attached).
    #[serde(default)]
    pub starred: bool,
    /// User dismissed the auto-star suggestion for this element.
    #[serde(default)]
    pub star_dismissed: bool,
    #[serde(default)]
    pub updated_at: String,
}

pub fn feedback_key(page_id: &str, element_id: &str) -> String {
    format!("{page_id}:{element_id}")
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

/// One global mutex around every store read→modify→write. With manifest
/// `concurrency` > 1 the host runs this plugin's calls on several wasm
/// instances at once; guest memory can't lock across them, so mutual
/// exclusion rides on the host store's atomic put-if-absent (peckboard ≥
/// 0.0.189). `Ok(None)` = contended — surface "busy" to the caller. `f`
/// must LOAD what it mutates inside the closure.
#[cfg(target_arch = "wasm32")]
pub fn try_with_store_lock<T>(f: impl FnOnce() -> Result<T, String>) -> Result<Option<T>, String> {
    let acquired = call_host(
        HostFn::StorePutIfAbsent,
        &json!({
            "collection": "locks",
            "key": "store",
            "data": { "at": clock() },
            // Above the 30s call budget generate calls can use, so a live
            // holder is never stolen; a trapped one leaks ≤ 60s.
            "ttl_secs": 60,
        }),
    )?
    .get("acquired")
    .and_then(|v| v.as_bool())
    .unwrap_or(false);
    if !acquired {
        return Ok(None);
    }
    let out = f();
    store_delete("locks", "store");
    out.map(Some)
}

/// Host builds (unit tests) are single-threaded pure logic — no lease.
#[cfg(not(target_arch = "wasm32"))]
pub fn try_with_store_lock<T>(f: impl FnOnce() -> Result<T, String>) -> Result<Option<T>, String> {
    f().map(Some)
}

/// The user-facing form of lease contention.
pub const BUSY_MSG: &str = "another ui-gauge update is in flight — try again in a moment";

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

/// Monotonic-enough id: clock second + atomic counter (sorts by time).
pub fn new_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let compact: String = clock().chars().filter(|c| c.is_ascii_digit()).collect();
    format!("{prefix}-{compact}-{n:04}")
}

// ── Loading ───────────────────────────────────────────────────────────

pub fn pages() -> Vec<Page> {
    let mut out: Vec<Page> = store_list(PAGES_COLLECTION)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, v)| serde_json::from_value(v).ok())
        .collect();
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    out
}

pub fn all_feedback() -> Vec<Feedback> {
    store_list(FEEDBACK_COLLECTION)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, v)| serde_json::from_value(v).ok())
        .collect()
}

pub fn delete_page(id: &str) {
    store_delete(PAGES_COLLECTION, id);
    store_delete(PAGE_HTML_COLLECTION, id);
    let prefix = format!("{id}:");
    for (k, _) in store_list(FEEDBACK_COLLECTION).unwrap_or_default() {
        if k.starts_with(&prefix) {
            store_delete(FEEDBACK_COLLECTION, &k);
        }
    }
    for (k, _) in store_list(SHOTS_COLLECTION).unwrap_or_default() {
        if k.starts_with(&prefix) {
            store_delete(SHOTS_COLLECTION, &k);
        }
    }
}

// ── Submission validation ─────────────────────────────────────────────

fn valid_element_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id != PAGE_ELEMENT_ID
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Validate a generated page submission. Pure — testable.
pub fn validate_submission(html: &str, elements: &[PageElement]) -> Result<(), String> {
    if html.len() > HTML_MAX_LEN {
        return Err(format!(
            "html too large ({} chars, max {HTML_MAX_LEN}) — trim the page and resubmit",
            html.len()
        ));
    }
    let lower = html.to_lowercase();
    if !lower.contains("<html") && !lower.contains("<body") {
        return Err("html must be a complete self-contained HTML document".into());
    }
    if lower.contains("<script") {
        return Err("no JavaScript allowed — the page is rendered in a script-less frame".into());
    }
    if elements.is_empty() {
        return Err("'elements' must list every marked element (at least one)".into());
    }
    if elements.len() > MAX_ELEMENTS {
        return Err(format!(
            "too many elements ({}, max {MAX_ELEMENTS}) — mark the meaningful regions, not every node",
            elements.len()
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for el in elements {
        if !valid_element_id(&el.id) {
            return Err(format!(
                "element id '{}' must be 1-64 chars of [a-z0-9-_] (and not '{PAGE_ELEMENT_ID}')",
                el.id
            ));
        }
        if !seen.insert(el.id.clone()) {
            return Err(format!("duplicate element id '{}'", el.id));
        }
        if el.label.trim().is_empty() {
            return Err(format!("element '{}' is missing a label", el.id));
        }
        if !html.contains(&format!("data-uig-id=\"{}\"", el.id)) {
            return Err(format!(
                "element '{}' is listed but data-uig-id=\"{}\" does not appear in the html",
                el.id, el.id
            ));
        }
    }
    Ok(())
}

// ── The preference prompt ─────────────────────────────────────────────

/// One line of the composed prompt plus where it came from — the UI shows
/// the ingredient list so the user can see exactly what feeds the prompt.
#[derive(Debug, Clone, Serialize)]
pub struct Ingredient {
    pub line: String,
    pub section: String, // "do" | "avoid" | "reference"
    pub page_id: String,
    pub page_name: String,
    pub element_id: String,
    pub element_label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrefPrompt {
    pub text: String,
    pub ingredients: Vec<Ingredient>,
}

fn clip(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    let cut: String = t.chars().take(max).collect();
    format!("{cut}…")
}

fn element_label<'a>(page: &'a Page, element_id: &str) -> Option<&'a str> {
    if element_id == PAGE_ELEMENT_ID {
        return Some("whole page");
    }
    page.elements
        .iter()
        .find(|e| e.id == element_id)
        .map(|e| e.label.as_str())
}

/// Compose the user's UI preference prompt from every piece of feedback.
/// Pure and computed on read — re-editing or deleting feedback changes the
/// very next read, so it can never go stale. `shot_keys` is the set of
/// feedback keys that actually have a stored screenshot (a star without a
/// captured shot contributes a Do-line but no reference).
pub fn preference_prompt(
    pages: &[Page],
    feedback: &[Feedback],
    shot_keys: &std::collections::BTreeSet<String>,
) -> PrefPrompt {
    let mut dos: Vec<Ingredient> = Vec::new();
    let mut avoids: Vec<Ingredient> = Vec::new();
    let mut refs: Vec<Ingredient> = Vec::new();

    for page in pages {
        for fb in feedback.iter().filter(|f| f.page_id == page.id) {
            let Some(label) = element_label(page, &fb.element_id) else {
                continue; // stale feedback for a removed element
            };
            let comment = clip(&fb.comment, 300);
            let make = |line: String, section: &str| Ingredient {
                line,
                section: section.into(),
                page_id: page.id.clone(),
                page_name: page.name.clone(),
                element_id: fb.element_id.clone(),
                element_label: label.to_string(),
            };
            match fb.verdict.as_str() {
                "up" => {
                    let line = if comment.is_empty() {
                        format!("- {label} (from \"{}\"): liked as generated.", page.name)
                    } else {
                        format!("- {label} (from \"{}\"): {comment}", page.name)
                    };
                    dos.push(make(line, "do"));
                }
                "down" => {
                    let line = if comment.is_empty() {
                        format!("- {label} (from \"{}\"): disliked as generated.", page.name)
                    } else {
                        format!("- {label} (from \"{}\"): {comment}", page.name)
                    };
                    avoids.push(make(line, "avoid"));
                }
                _ => {
                    // No verdict — a bare comment still carries signal.
                    if !comment.is_empty() {
                        let line = format!("- {label} (from \"{}\"): {comment}", page.name);
                        dos.push(make(line, "do"));
                    }
                }
            }
            if fb.starred && shot_keys.contains(&feedback_key(&fb.page_id, &fb.element_id)) {
                let id = feedback_key(&fb.page_id, &fb.element_id);
                let line = format!("- id \"{id}\" — {label} (from \"{}\")", page.name);
                refs.push(make(line, "reference"));
            }
        }
    }

    let mut text = String::from(
        "## UI taste (ui-gauge)\nThe user has reviewed generated UI and recorded these \
         preferences. Apply them whenever you build, modify, or judge user interface for \
         this user.\n",
    );
    if dos.is_empty() && avoids.is_empty() && refs.is_empty() {
        text.push_str("\n(No recorded preferences yet.)\n");
    }
    if !dos.is_empty() {
        text.push_str("\n### Do\n");
        for i in &dos {
            text.push_str(&i.line);
            text.push('\n');
        }
    }
    if !avoids.is_empty() {
        text.push_str("\n### Avoid\n");
        for i in &avoids {
            text.push_str(&i.line);
            text.push('\n');
        }
    }
    if !refs.is_empty() {
        text.push_str(
            "\n### Visual references\nBefore designing similar UI, fetch these screenshots \
             of elements the user starred with the ui_gauge_reference_image tool:\n",
        );
        for i in &refs {
            text.push_str(&i.line);
            text.push('\n');
        }
    }

    let mut ingredients = dos;
    ingredients.extend(avoids);
    ingredients.extend(refs);
    PrefPrompt { text, ingredients }
}

/// The block injected into sessions of an enabled folder. None when there is
/// nothing to say yet — an empty taste block would be pure noise.
pub fn session_block(pref: &PrefPrompt) -> Option<String> {
    if pref.ingredients.is_empty() {
        return None;
    }
    Some(pref.text.clone())
}

// ── The generation prompt ─────────────────────────────────────────────

/// The prompt handed to a generation session: the user's accumulated taste,
/// what earlier pages covered, the brief (or free choice), and the
/// submission contract with element marking.
pub fn build_generation_prompt(brief: &str, pages: &[Page], feedback: &[Feedback]) -> String {
    let pref = preference_prompt(pages, feedback, &Default::default());
    let mut p = String::from(
        "Design ONE self-contained UI page as a single HTML document with ALL CSS inlined \
         in a <style> tag. No external resources, no JavaScript — it is rendered statically \
         in a script-less frame where the user reviews it element by element.\n\n",
    );
    let brief = brief.trim();
    if brief.is_empty() {
        p.push_str(
            "## The page\nNo brief was given — pick ONE realistic, representative screen \
             yourself (dashboard, settings, list + detail, form, onboarding, …) and invent \
             plausible content. Prefer a page type the previous pages below have not covered.\n\n",
        );
    } else {
        p.push_str(&format!(
            "## The page\nThe user asked for: {brief}\nInvent plausible content around that.\n\n"
        ));
    }
    p.push_str(&pref.text);
    if !pages.is_empty() {
        p.push_str("\n## Previously generated pages\n");
        for pg in pages.iter().rev().take(10) {
            let brief_note = if pg.brief.trim().is_empty() {
                String::new()
            } else {
                format!(" (brief: {})", clip(&pg.brief, 80))
            };
            p.push_str(&format!("- {}{brief_note}\n", pg.name));
        }
        p.push_str(
            "Explore ground these have not covered rather than repeating them, while \
             keeping every Do-preference above.\n",
        );
    }
    p.push_str(&format!(
        "\n## Marking elements\nMark every meaningful region of the page (header, nav, each \
         card or panel, key controls — the pieces a user would want to praise or reject \
         individually, at most {MAX_ELEMENTS}) by putting BOTH attributes on its outermost \
         tag:\n\
         - data-uig-id: a unique kebab-case id, e.g. \"metric-cards\"\n\
         - data-uig-label: a short human label, e.g. \"Metric summary cards\"\n\n\
         ## Submitting\nSubmit EXACTLY ONE result by calling the ui_gauge_submit_page tool \
         with:\n\
         - html: the complete HTML document (≤ {HTML_MAX_LEN} chars)\n\
         - elements: the full list of marked elements as {{id, label, kind}} — it must match \
         the data-uig-id attributes in the html exactly\n\
         - name: a short title for the page\n\
         - design_notes: 2-4 sentences on the design direction you chose and why\n\
         Do not ask questions; do not produce anything else.",
    ));
    p
}

// ── Generation state (engine/generation) ──────────────────────────────

pub fn generation_state() -> Value {
    store_get(ENGINE_COLLECTION, "generation")
        .ok()
        .flatten()
        .unwrap_or_else(|| json!({ "status": "idle" }))
}

pub fn set_generation_state(v: Value) {
    let _ = store_put(ENGINE_COLLECTION, "generation", v);
}

/// One-time sweep of the 0.2.x collections (rubric/baselines/evaluations) —
/// the 0.3.0 model replaces them wholesale. Runs from `timer.tick` once.
pub fn sweep_legacy_collections() {
    let marker = "swept_v3";
    if store_get(ENGINE_COLLECTION, marker)
        .ok()
        .flatten()
        .is_some()
    {
        return;
    }
    for coll in [
        "categories",
        "baselines",
        "baseline_images",
        "baseline_html",
        "evaluations",
    ] {
        for (k, _) in store_list(coll).unwrap_or_default() {
            store_delete(coll, &k);
        }
    }
    let _ = store_put(ENGINE_COLLECTION, marker, json!({ "at": clock() }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn page(id: &str, name: &str, elems: &[(&str, &str)]) -> Page {
        Page {
            id: id.into(),
            name: name.into(),
            brief: String::new(),
            model: String::new(),
            design_notes: String::new(),
            created_at: id.into(),
            elements: elems
                .iter()
                .map(|(eid, label)| PageElement {
                    id: eid.to_string(),
                    label: label.to_string(),
                    kind: String::new(),
                })
                .collect(),
        }
    }

    fn fb(
        page_id: &str,
        element_id: &str,
        verdict: &str,
        comment: &str,
        starred: bool,
    ) -> Feedback {
        Feedback {
            page_id: page_id.into(),
            element_id: element_id.into(),
            verdict: verdict.into(),
            comment: comment.into(),
            starred,
            star_dismissed: false,
            updated_at: String::new(),
        }
    }

    #[test]
    fn validate_accepts_a_marked_page() {
        let elems = vec![PageElement {
            id: "hero".into(),
            label: "Hero".into(),
            kind: String::new(),
        }];
        let html =
            "<html><body><div data-uig-id=\"hero\" data-uig-label=\"Hero\">x</div></body></html>";
        assert!(validate_submission(html, &elems).is_ok());
    }

    #[test]
    fn validate_rejects_scripts_missing_ids_and_duplicates() {
        let el = |id: &str| PageElement {
            id: id.into(),
            label: "L".into(),
            kind: String::new(),
        };
        let html = "<html><body><div data-uig-id=\"a\">x</div></body></html>";
        assert!(validate_submission(html, &[el("a")]).is_ok());
        assert!(
            validate_submission("<html><script>x</script></html>", &[el("a")]).is_err(),
            "script must be rejected"
        );
        assert!(
            validate_submission(html, &[el("b")]).is_err(),
            "id not present in html"
        );
        assert!(
            validate_submission(html, &[el("a"), el("a")]).is_err(),
            "duplicate ids"
        );
        assert!(
            validate_submission(html, &[el("_page")]).is_err(),
            "reserved id"
        );
        assert!(validate_submission(html, &[]).is_err(), "no elements");
    }

    #[test]
    fn preference_prompt_sorts_feedback_into_sections() {
        let pages = vec![page(
            "p1",
            "Dashboard",
            &[("hero", "Hero"), ("nav", "Sidebar nav")],
        )];
        let feedback = vec![
            fb("p1", "hero", "up", "bold type, lots of whitespace", false),
            fb("p1", "nav", "down", "too cramped", false),
            fb("p1", "_page", "", "overall direction is right", false),
        ];
        let pref = preference_prompt(&pages, &feedback, &BTreeSet::new());
        assert!(pref.text.contains("### Do"), "{}", pref.text);
        assert!(pref.text.contains("bold type"), "{}", pref.text);
        assert!(pref.text.contains("### Avoid"), "{}", pref.text);
        assert!(pref.text.contains("too cramped"), "{}", pref.text);
        assert!(pref.text.contains("whole page"), "{}", pref.text);
        assert!(!pref.text.contains("Visual references"), "{}", pref.text);
        assert_eq!(pref.ingredients.len(), 3);
    }

    #[test]
    fn starred_elements_become_references_only_with_a_shot() {
        let pages = vec![page("p1", "Dashboard", &[("hero", "Hero")])];
        let feedback = vec![fb("p1", "hero", "up", "", true)];
        let none = preference_prompt(&pages, &feedback, &BTreeSet::new());
        assert!(!none.text.contains("Visual references"), "{}", none.text);
        let mut shots = BTreeSet::new();
        shots.insert("p1:hero".to_string());
        let some = preference_prompt(&pages, &feedback, &shots);
        assert!(some.text.contains("Visual references"), "{}", some.text);
        assert!(
            some.text.contains("ui_gauge_reference_image"),
            "{}",
            some.text
        );
        assert!(some.text.contains("id \"p1:hero\""), "{}", some.text);
    }

    #[test]
    fn preference_prompt_updates_when_feedback_changes() {
        let pages = vec![page("p1", "Dashboard", &[("hero", "Hero")])];
        let mut feedback = vec![fb("p1", "hero", "up", "keep this", false)];
        assert!(
            preference_prompt(&pages, &feedback, &BTreeSet::new())
                .text
                .contains("keep this")
        );
        feedback[0].verdict = "down".into();
        let p = preference_prompt(&pages, &feedback, &BTreeSet::new());
        assert!(p.text.contains("### Avoid"), "{}", p.text);
        assert!(!p.text.contains("### Do\n- Hero"), "{}", p.text);
    }

    #[test]
    fn session_block_is_none_until_there_is_feedback() {
        let pages = vec![page("p1", "Dashboard", &[("hero", "Hero")])];
        let empty = preference_prompt(&pages, &[], &BTreeSet::new());
        assert!(session_block(&empty).is_none());
        let with = preference_prompt(
            &pages,
            &[fb("p1", "hero", "up", "", false)],
            &BTreeSet::new(),
        );
        assert!(session_block(&with).is_some());
    }

    #[test]
    fn stale_feedback_for_removed_elements_is_ignored() {
        let pages = vec![page("p1", "Dashboard", &[("hero", "Hero")])];
        let feedback = vec![fb("p1", "gone", "up", "orphan", false)];
        let pref = preference_prompt(&pages, &feedback, &BTreeSet::new());
        assert!(pref.ingredients.is_empty(), "{}", pref.text);
    }

    #[test]
    fn generation_prompt_carries_brief_taste_history_and_contract() {
        let pages = vec![page("p1", "Dashboard", &[("hero", "Hero")])];
        let feedback = vec![
            fb("p1", "hero", "up", "bold type", false),
            fb("p1", "hero", "down", "", false),
        ];
        let p = build_generation_prompt("a settings page", &pages, &feedback);
        assert!(p.contains("a settings page"), "{p}");
        assert!(p.contains("bold type"), "{p}");
        assert!(p.contains("Previously generated pages"), "{p}");
        assert!(p.contains("Dashboard"), "{p}");
        assert!(p.contains("data-uig-id"), "{p}");
        assert!(p.contains("data-uig-label"), "{p}");
        assert!(p.contains("ui_gauge_submit_page"), "{p}");
        assert!(p.contains("design_notes"), "{p}");

        let free = build_generation_prompt("  ", &pages, &feedback);
        assert!(free.contains("No brief was given"), "{free}");
    }

    #[test]
    fn clip_truncates_long_comments() {
        assert_eq!(clip("  hi  ", 10), "hi");
        let long = "x".repeat(400);
        let c = clip(&long, 300);
        assert!(c.chars().count() == 301 && c.ends_with('…'));
    }
}
