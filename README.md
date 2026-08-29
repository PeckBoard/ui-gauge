# Peckboard UI-Gauge Plugin

Gauge UI design quality against **user-ranked baselines** — and grow those
baselines with a **generate → rate → learn loop**: one button generates the
next baseline UI, you rate it 1-10 per category, and every change you rate
high becomes part of a living **overall baseline prompt** that agents apply
whenever they build or judge UI for you.

## The Generation Loop (0.2.0)

1. **Generate** — pick a folder + model and press *Generate baseline*. The
   plugin spawns a temp agent session whose prompt carries the current
   overall baseline prompt, the low-rated changes to avoid, and a contract:
   design ONE self-contained HTML page (inline CSS, no JS), change one
   aspect vs the previous baselines, and submit it via
   `ui_gauge_submit_baseline` with a **change_summary** — a reusable style
   directive describing what changed.
2. **Rate** — the submitted page renders in a script-less sandboxed frame in
   the gallery; rank it 1-10 per category like any baseline.
3. **Learn** — average **7+** graduates the change_summary into the overall
   baseline prompt; **4−** lists it as something to avoid next generation.
   The overall prompt is recomputed on every read — re-rating or deleting a
   baseline updates it instantly, so it is always current. Agents receive it
   from `ui_gauge_rubric`.

## Scoring Worker Output

1. **Calibrate** — on the UI Gauge page, upload baseline screenshots
   (downscaled client-side to fit storage) and rank each 1-10 per category.
   Default categories: visual hierarchy, spacing & alignment, typography,
   color & contrast, consistency with the app, accessibility — all editable,
   each with a **bar** that defaults to the median of your rankings
   (override per category).
2. **Score** — a vision-capable agent calls `ui_gauge_rubric` (categories,
   bars, your rankings), fetches an anchor or two with
   `ui_gauge_baseline_image`, scores the target UI's screenshots on YOUR
   scale, and submits via `ui_gauge_score`.
3. **Enforce** — any category below its bar makes the verdict `subpar` and
   creates one card per gap in the caller's project ("UI: improve spacing …
   — scored 4, bar 7"). Evaluations land in the page history with score
   trends. Pairs with the session-control orchestrator's UX standard: the
   goal stays not-done until the gauge passes.

The plugin owns the rubric, calibration, verdicts, history, and cards; it
never looks at pixels itself (WASM has no vision) — the agent is the eyes.

## Tools

| Tool | What it does |
| ---- | ------------ |
| `ui_gauge_rubric` | Categories with bars, the user's baseline rankings + change prompts, and the overall baseline prompt. |
| `ui_gauge_baseline_image` | One baseline screenshot (base64) by id. |
| `ui_gauge_submit_baseline` | Submit a generated baseline (HTML + change_summary); rated by the user, high ratings feed the overall prompt. |
| `ui_gauge_score` | Submit per-category 1-10 scores → pass/subpar verdict + gap list; subpar auto-creates cards. |
| `ui_gauge_history` | Recent evaluations, newest first. |

## Permissions

- `provide_mcp_tools` — the five tools above.
- `data_store` — rubric, baselines, generated HTML, evaluation history.
- `user_authority` + `contribute_sidebar` — the UI Gauge page.
- `session_write` + `session_dispatch` — the temp generation session.
- `models_read` — the generation model picker.

Hooks: `mcp.tool.invoke`, `timer.tick` (clock only — WASM has no time
source), `session.agent.ended` (marks a generation that ended without
submitting), `http.request.before` / `http.request.authed` (the page).

## Build

```bash
./build.sh
# → target/wasm32-unknown-unknown/release/peckboard_ui_gauge_plugin.wasm
```

Drop the `.wasm` into `<dataDir>/plugins/ui-gauge.wasm` and restart, or
install via the plugin registry (id `ui-gauge`).
