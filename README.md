# Peckboard UI-Gauge Plugin

Gauge UI design quality against **user-ranked baselines**. You upload
reference screenshots on the **UI Gauge** sidebar page and rank each 1-10
per category; agents then score worker output on that same calibrated
scale, and subpar results automatically become follow-up cards.

## How It Works

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
| `ui_gauge_rubric` | Categories with bars + the user's baseline rankings (calibration anchors). |
| `ui_gauge_baseline_image` | One baseline screenshot (base64) by id. |
| `ui_gauge_score` | Submit per-category 1-10 scores → pass/subpar verdict + gap list; subpar auto-creates cards. |
| `ui_gauge_history` | Recent evaluations, newest first. |

## Permissions

- `provide_mcp_tools` — the four tools above.
- `data_store` — rubric, baselines, evaluation history.
- `user_authority` + `contribute_sidebar` — the UI Gauge page.

Hooks: `mcp.tool.invoke`, `timer.tick` (clock only — WASM has no time
source), `http.request.before` / `http.request.authed` (the page).

## Build

```bash
./build.sh
# → target/wasm32-unknown-unknown/release/peckboard_ui_gauge_plugin.wasm
```

Drop the `.wasm` into `<dataDir>/plugins/ui-gauge.wasm` and restart, or
install via the plugin registry (id `ui-gauge`).
