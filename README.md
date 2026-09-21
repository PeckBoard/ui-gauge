# Peckboard UI-Gauge Plugin

Steer UI-generating agents toward **the user's taste** with a
**generate → review → learn → attach** loop:

1. **Generate** — on the UI Gauge page, pick a folder + model, optionally
   type a brief ("a settings page" — empty means the agent picks a
   representative screen), and press *Generate page*. The plugin spawns a
   temp agent session whose prompt carries every preference recorded so
   far plus a contract: design ONE self-contained HTML page (inline CSS,
   no JS), mark every meaningful region with `data-uig-id` /
   `data-uig-label`, and submit it via `ui_gauge_submit_page` with the
   matching element manifest.
2. **Review** — the page renders in a script-less same-origin frame with
   numbered pins over every marked element (document-review style). Each
   element takes a 👍/👎, a comment ("why" — the comment becomes a prompt
   line), and a ★ star. Liked elements get an auto-star suggestion; a
   starred element is screenshotted client-side (SVG foreignObject →
   canvas, downscaled) and stored as a visual reference.
3. **Learn** — the **UI preference prompt** is recomputed on every read
   from all feedback: 👍 lines become *Do* directives, 👎 lines become
   *Avoid* directives, starred elements are listed as fetchable visual
   references. The page shows the composed prompt, exactly what feeds
   each line, and a copy button.
4. **Attach** — toggle the prompt per **folder**. Chat sessions in an
   enabled folder receive the block as a session system prompt on their
   next turn (`session.message.before` → `set_session_system_prompt`,
   hash-guarded — the graphify pattern); toggling off removes it the
   same way. Workers are not covered: core does not fire the hook for
   worker sessions — they can call `ui_gauge_prefs` instead.

## Tools

| Tool | What it does |
| ---- | ------------ |
| `ui_gauge_submit_page` | Submit a generated page: html + element manifest + name + design notes. |
| `ui_gauge_prefs` | The composed preference prompt + starred reference list. |
| `ui_gauge_reference_image` | One starred element's screenshot (base64) by id. |
| `ui_gauge_history` | Generated pages with their per-element feedback, newest first. |

## Permissions

- `provide_mcp_tools` — the four tools above.
- `data_store` — pages, per-element feedback, screenshots, folder toggles.
- `user_authority` + `contribute_sidebar` — the UI Gauge page.
- `session_write` + `session_dispatch` — the temp generation session.
- `session_prompt_write` — attach/remove the preference block on sessions.
- `models_read` — the generation model picker.

Hooks: `mcp.tool.invoke`, `timer.tick` (clock only — WASM has no time
source; also runs the one-time 0.2.x data sweep), `session.agent.ended`
(marks a generation that ended without submitting),
`session.message.before` (the per-turn prompt sync),
`http.request.before` / `http.request.authed` (the page).

## Build

```bash
./build.sh
# → target/wasm32-unknown-unknown/release/peckboard_ui_gauge_plugin.wasm
```

Drop the `.wasm` into `<dataDir>/plugins/ui-gauge.wasm` and restart, or
install via the plugin registry (id `ui-gauge`).

## Upgrading from 0.2.x

0.3.0 replaces the rubric/score/baseline model wholesale. The old
collections (categories, baselines, baseline images/html, evaluations)
are deleted by a one-time sweep on the first timer tick after upgrade.
