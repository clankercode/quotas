# MiniMax fixtures and TUI — update to match current payload

**Status:** design approved (brainstorming sections 1–4)
**Date:** 2026-07-11
**Scope:** `src/providers/minimax.rs`, `src/tui/dashboard.rs`, `src/config.rs`, `src/main.rs`, `tests/fixtures/minimax/`, `CLAUDE.md`, `screenshots/`

## Context

The MiniMax provider currently parses a count-based payload:

- `model_remains[*].current_interval_total_count` (limit)
- `model_remains[*].current_interval_usage_count` (REMAINING — inverted-naming quirk, not consumed)
- `model_remains[*].current_weekly_total_count` / `current_weekly_usage_count` (same shape)

The current live payload observed in the user's `~/.cache/quotas/cache.json` differs:

1. Model names changed: `MiniMax-M2.7` / coding-plan-* → `general`, `video`.
2. New fields: `current_interval_remaining_percent`, `current_interval_status`, `current_weekly_remaining_percent`, `current_weekly_status`.
3. `total_count` is `0` for the `general` model — the parser currently gates on `total_count > 0`, so it silently produces **zero windows** for this model and the TUI renders nothing meaningful.
4. The `video` model still reports counts (`3/3` remaining → 100%).
5. The `render_minimax_windows` paired-row renderer, the 2-col span, the `vertical_spanning` config, and `natural_card_height`/`card_weight` MiniMax branches all assume one model → 5h+wk → one row, which is the "wide card hack" that no longer fits the more-traditional payload.

User direction (recorded during brainstorming):
- Show `general` as interval + weekly, derived from `*_remaining_percent` (ignore total=0).
- Show `video` as interval + weekly, derived from counts.
- Always render the bar, even when fully depleted (status==0 + pct==0).
- Drop the wide MiniMax hack; treat MiniMax like a normal provider.

## Parser changes — `src/providers/minimax.rs`

### New `ModelRemain` fields

```rust
#[serde(default)]
current_interval_remaining_percent: Option<u8>,
#[serde(default)]
current_interval_status: Option<i32>,
#[serde(default)]
current_weekly_remaining_percent: Option<u8>,
#[serde(default)]
current_weekly_status: Option<i32>,
```

(All four default-tolerant; absent → no percent-derived window.)

### Per-model window emission

For each model, emit at most one interval window and one weekly window:

**Interval window (was `5h/<short>`):**
1. If `total_count > 0` → existing logic: `limit = total_count`, `remaining = usage_count.clamp(0, limit)`, `used = limit - remaining`, label `5h/<short>`, period default 18000s.
2. Else if `Some(pct) = current_interval_remaining_percent` → `limit = 100`, `remaining = pct.min(100)`, `used = (100 - pct).clamp(0, 100)`, label `5h/<short>`, period 18000s.
3. Else → no interval window.

**Weekly window (was `wk/<short>`):** same three-tier rule using `weekly_total`, `weekly_usage`, `current_weekly_remaining_percent`, default period `7 * 86400`.

**Status:** `current_interval_status` / `current_weekly_status` are kept on the struct (so the fields are read and deserialized) but **do not** gate rendering. Policy: always render the bar. Future use (depleted badges etc.) is out of scope.

### `plan_name` resolution

Three-tier priority:
1. First model whose name (lowercased) starts with `minimax-m` or `coding-plan`. Use its `model_name`.
2. Else, if any model is present, use the literal string `"MiniMax Coding Plan"` (static fallback).
3. Else, `"MiniMax Coding Plan"` (no models at all).

Output format stays `format!("MiniMax · {plan}", plan = …)`.

### `short_model_name`

No alias logic change needed for the new payload (`general`, `video` pass through unchanged). Existing `Hailuo-2.3-*` / `coding-plan-*` / `lyrics_generation` aliases remain — harmless no-ops on current data.

## TUI changes — drop the wide MiniMax hack

### `src/tui/dashboard.rs`

- **Delete** `render_minimax_windows` and `minimax_bar_cell` (Dashboard private fns).
- **Delete** the inline `if result.kind == ProviderKind::Minimax { render_minimax_windows(...); return; }` early-return branch in `render_entry`.
- **Delete** the `is_minimax` check in `flow_placements` (sets `span` and `row_span`). MiniMax gets the default `span = 1, row_span = 1`.
- **Delete** the `r.kind == ProviderKind::Minimax` arm in `natural_card_height`. MiniMax falls through to the two-line-per-window calculation.
- **Delete** the `r.kind == ProviderKind::Minimax` arm in `card_weight`. MiniMax gets the full `visible` window count.
- No other code in `render_entry` changes — the generic window rendering already supports multiple windows and section headers.

### `src/config.rs` and `src/main.rs`

- **Delete** `pub vertical_spanning: bool` from `UIConfig`.
- **Delete** the `vertical_spanning` CLI flag parsing in `main.rs` and its write to `dashboard.vertical_spanning`.

### `CLAUDE.md`

- **Delete** the `vertical_spanning = true` line in the example config block.
- **Delete** any commentary about MiniMax-as-2×2 in the surrounding prose (the surrounding paragraph's mention of "MiniMax as 2x2 card" becomes inaccurate).

### `README.md`

- No change expected (the table entry for MiniMax references the endpoint, not the layout).

## Fixtures — `tests/fixtures/minimax/`

### `coding_plan_remains_live.json`

Captured from a live fetch, anonymized:

- `base_resp.status_code = 0`, `base_resp.status_msg = ""`.
- Two entries in `model_remains`:
  - `model_name: "general"`, all count fields `0`, `current_interval_remaining_percent: 99`, `current_interval_status: 1`, `current_weekly_remaining_percent: 98`, `current_weekly_status: 1`.
  - `model_name: "video"`, `current_interval_total_count: 3`, `current_interval_usage_count: 3`, `current_weekly_total_count: 21`, `current_weekly_usage_count: 21`, all percents `100`, all statuses `1`.
- Timestamps in the JSON file are stored at illustrative anchor values (5h interval window around `T0`, weekly window roughly spanning a week). The test re-anchors all four timestamps at load time so `reset_at` and `period_seconds` stay deterministic without the fixture rotting.

Test code: read fixture, deserialize, set `start_time`/`end_time`/`weekly_*_time` to known values via `body["model_remains"][*]["start_time"] = …` before calling `parse_response`. Documented in test comments.

### Tests in `src/providers/minimax.rs`

Move to a fixture-driven pattern modeled on `src/providers/grok.rs::tests`:

```rust
fn fixture(name: &str) -> serde_json::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/minimax")
        .join(name);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read fixture {}: {}", path.display(), e));
    serde_json::from_str(&raw).expect("parse fixture json")
}
```

Test list:
- `parses_live_fixture_mixed_count_and_percent` — loads `coding_plan_remains_live.json`, anchors timestamps at `Utc::now()`, asserts:
  - `quota.plan_name == "MiniMax · MiniMax Coding Plan"` (static fallback fires).
  - `quota.windows.len() == 4`.
  - Two windows for `general`: `used ≈ 1`, `remaining = 99`, `limit = 100` for interval; `used ≈ 2`, `remaining = 98`, `limit = 100` for weekly.
  - Two windows for `video`: `used = 0`, `remaining = 3`, `limit = 3` for interval; `used = 0`, `remaining = 21`, `limit = 21` for weekly.
  - Coding-plan prioritization: video or general sorted first per current `is_coding` logic — for the new payload, neither matches, so order is preserved as in the fixture (general first).
- `depleted_window_status_zero_still_renders` — synthetic body with `current_interval_status: 0`, `current_interval_remaining_percent: 0`, `total_count: 0` → asserts a window with `used = 100, remaining = 0, limit = 100` exists.
- `unknown_models_fall_back_to_static_plan_name` — synthetic body with only `image-01` model → asserts `quota.plan_name == "MiniMax · MiniMax Coding Plan"`.
- `parses_count_based_payload_retrocompat` — covered by the existing `parses_minimax_remains_payload` test in `src/providers/minimax.rs::tests` (count-based `MiniMax-M2.7` body with `used=13, remaining=187, limit=200` expectations). It runs against the count branch unchanged and pins the regression boundary.

## Verification

Run, in order, from a fresh worktree:

```bash
cargo build                                            # 0 warnings
cargo test                                             # all green, new + legacy
cargo clippy -- -D warnings                            # clean
cargo run -- --snap --snap-width 160 --snap-height 50 \
    --snap-output screenshots/before-bpayload.txt      # baseline
# ... code changes ...
cargo run -- --snap --snap-width 160 --snap-height 50 \
    --snap-output screenshots/after-bpayload.txt       # post-change
just screenshots-multi                                 # 5-size grid capture
```

Visual review:
- `MiniMax` card sits in the normal 1-col flow (no 2-col span, no 2×2 when `vertical_spanning` is gone).
- 4 windows render with section headers if ≥6 windows (it doesn't, so no headers).
- `general` and `video` rows show bars derived from the percent field (99%/98%) and counts (100%) respectively.

`screenshots/log.md` gets an "Iter N" entry describing the layout diff and the bar values for MiniMax.

## Out of scope

- Sorting change for non-coding models in `is_coding` — current logic is preserved verbatim. New naming (`general`/`video`) doesn't trigger coding priority and is left in fixture order.
- Custom MiniMax detail view (section headers per model) — deferred per brainstorming option C.
- Per-model status badge / depleted indicator — `*_status` is captured but unused for rendering.
- Removing `Hailuo-2.3-*` / `coding-plan-*` / `lyrics_generation` aliases — they're harmless no-ops on the current payload and may match future API entries.

## Open assumptions

1. Cache.json reflects the live payload shape; not a transient error page. (Verified: `base_resp.status_code == 0`, two models, populated fields.)
2. `*_status = 1` means "available/normal"; `= 0` means depleted. (Inferred; field stays informational.)
3. The user's plan is "MiniMax Coding Plan". (Confirmed in clarifying question.)
