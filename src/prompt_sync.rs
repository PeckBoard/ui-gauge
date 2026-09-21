//! Keeps each chat session's UI-taste block in sync with its folder's
//! toggle. `session.message.before` fires once per user turn (chat sessions
//! only — core never fires it for workers); the job here is to write the
//! block when the folder is enabled, take it back off when it is not, and
//! otherwise do nothing at all — the host call only happens when the text
//! actually changed. Same shape as the graphify plugin's prompt sync.

use serde_json::{Value, json};

use crate::gauge;
use crate::host::{HostFn, call_host};

/// Stable 32-bit FNV-1a, hex. Only ever compared against itself, so the
/// point is determinism across runs, not collision resistance.
pub fn hash_block(text: &str) -> String {
    let mut h: u32 = 0x811c9dc5;
    for b in text.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("{h:08x}")
}

pub fn folder_enabled(folder_id: &str) -> bool {
    gauge::store_get(gauge::FOLDER_PREFS_COLLECTION, folder_id)
        .ok()
        .flatten()
        .and_then(|v| v.get("enabled").and_then(|e| e.as_bool()))
        .unwrap_or(false)
}

/// Decide what a session's block should be. Pure — testable. `None` means
/// "no block" (disabled folder, or nothing to say yet).
pub fn desired_block(enabled: bool, pref: &gauge::PrefPrompt) -> Option<String> {
    if !enabled {
        return None;
    }
    gauge::session_block(pref)
}

/// Called from `session.message.before`. Never blocks a turn: every step
/// degrades to "leave the prompt alone".
pub fn sync(payload: &Value) {
    let Some(session_id) = payload
        .get("session_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
    else {
        return;
    };

    let folder_id = call_host(HostFn::CallerScope, &json!({}))
        .ok()
        .and_then(|s| {
            s.get("folder_id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    if folder_id.is_empty() {
        return;
    }

    let block = if folder_enabled(&folder_id) {
        let pages = gauge::pages();
        let feedback = gauge::all_feedback();
        let shots: std::collections::BTreeSet<String> = gauge::store_list(gauge::SHOTS_COLLECTION)
            .unwrap_or_default()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        desired_block(true, &gauge::preference_prompt(&pages, &feedback, &shots))
    } else {
        None
    };

    match block {
        Some(text) => write_prompt(session_id, &text),
        None => clear_prompt(session_id),
    }
}

fn write_prompt(session_id: &str, text: &str) {
    let hash = hash_block(text);
    let prev = gauge::store_get(gauge::SESSION_PROMPT_COLLECTION, session_id)
        .ok()
        .flatten();
    if prev
        .as_ref()
        .and_then(|v| v.get("hash").and_then(|h| h.as_str()))
        == Some(hash.as_str())
    {
        return; // already current — no host call
    }
    if call_host(
        HostFn::SetSessionSystemPrompt,
        &json!({ "session_id": session_id, "system_prompt": text }),
    )
    .is_ok()
    {
        let _ = gauge::store_put(
            gauge::SESSION_PROMPT_COLLECTION,
            session_id,
            json!({ "hash": hash, "at": gauge::clock() }),
        );
    }
}

fn clear_prompt(session_id: &str) {
    let prev = gauge::store_get(gauge::SESSION_PROMPT_COLLECTION, session_id)
        .ok()
        .flatten();
    let had = prev
        .as_ref()
        .and_then(|v| v.get("hash").and_then(|h| h.as_str()))
        .is_some_and(|h| !h.is_empty());
    if !had {
        return; // nothing of ours is set
    }
    if call_host(
        HostFn::SetSessionSystemPrompt,
        &json!({ "session_id": session_id }),
    )
    .is_ok()
    {
        let _ = gauge::store_put(
            gauge::SESSION_PROMPT_COLLECTION,
            session_id,
            json!({ "hash": "", "at": gauge::clock() }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gauge::{Feedback, Page, PageElement, preference_prompt};
    use std::collections::BTreeSet;

    fn pref_with_feedback() -> gauge::PrefPrompt {
        let pages = vec![Page {
            id: "p1".into(),
            name: "Dash".into(),
            brief: String::new(),
            model: String::new(),
            design_notes: String::new(),
            created_at: String::new(),
            elements: vec![PageElement {
                id: "hero".into(),
                label: "Hero".into(),
                kind: String::new(),
            }],
        }];
        let fb = vec![Feedback {
            page_id: "p1".into(),
            element_id: "hero".into(),
            verdict: "up".into(),
            comment: "nice".into(),
            starred: false,
            star_dismissed: false,
            updated_at: String::new(),
        }];
        preference_prompt(&pages, &fb, &BTreeSet::new())
    }

    #[test]
    fn desired_block_requires_toggle_and_content() {
        let pref = pref_with_feedback();
        assert!(desired_block(false, &pref).is_none(), "disabled folder");
        assert!(desired_block(true, &pref).is_some(), "enabled + content");
        let empty = preference_prompt(&[], &[], &BTreeSet::new());
        assert!(desired_block(true, &empty).is_none(), "nothing to say yet");
    }

    #[test]
    fn hash_is_stable_and_content_sensitive() {
        assert_eq!(hash_block("abc"), hash_block("abc"));
        assert_ne!(hash_block("abc"), hash_block("abd"));
        assert_eq!(hash_block("abc").len(), 8);
    }
}
