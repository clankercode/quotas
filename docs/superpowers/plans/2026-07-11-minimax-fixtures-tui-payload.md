# MiniMax Fixtures + TUI Update Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Update the MiniMax provider parser, fixtures, and TUI so MiniMax correctly displays its current API payload, and remove the wide-MiniMax spanning hack that no longer fits the new payload shape.

**Architecture:** TDD-first parser updates with new `*_remaining_percent`/`*_status` fields and a static plan-name fallback; live fixture captures the current payload; surgical removals from `dashboard.rs` and `config.rs` drop the `vertical_spanning` plumbing; screenshot diff verifies the visual impact.

**Tech Stack:** Rust 2021 (cargo, ratatui, serde, chrono), JSON fixtures, ratatui TestBackend-based snapshot testing already used by `src/tui/detail.rs::tests`.

**Spec:** `docs/superpowers/specs/2026-07-11-minimax-fixtures-tui-payload-design.md`

## Global Constraints

- Build with `cargo build -j 2` and `cargo test -j 2` per Max's `CLAUDE.md` system etiquette (2 threads max).
- `cargo clippy -- -D warnings` must pass before any task is committed.
- All file paths relative to the worktree root (`/home/xertrov/src/quotas/.worktrees/minimax-payload-update`).
- Use the existing `chrono::Utc` and `serde::Deserialize` patterns; no new dependencies.
- Do not touch `target/`, `screenshots/`, or anything outside the files explicitly named below except for screenshots captured in Task 1 and Task 8.
- Preserve the `MiniMax` display label (provider name string) and the `MINIMAX_API_KEY` env-var name; this task renames no public identifiers.

---

## Task 1: Set up worktree and capture before-snap baseline

**Files:** none created in the worktree (new branch only); `screenshots/before-bpayload.txt` will be written.

**Interfaces:** none — pure setup.

- [ ] **Step 1: Create worktree on a new branch**

```bash
cd /home/xertrov/src/quotas && git worktree add .worktrees/minimax-payload-update -b minimax-payload-update
```

Expected: branch created and worktree checked out at `.worktrees/minimax-payload-update`.

- [ ] **Step 2: Confirm baseline build works in the worktree**

```bash
cd /home/xertrov/src/quotas/.worktrees/minimax-payload-update && cargo build -j 2
```

Expected: `Finished` with no errors, exit code 0.

- [ ] **Step 3: Capture before-snap at 160x50**

```bash
mkdir -p screenshots && cd .worktrees/minimax-payload-update && \
  cargo run -- --snap --snap-width 160 --snap-height 50 \
    --snap-output ../../screenshots/before-bpayload.txt
```

Expected: file `screenshots/before-bpayload.txt` exists (top-level, outside worktree), ends with no panic.

- [ ] **Step 4: Commit nothing yet — Task 8 captures the visual diff.**

---

## Task 2: Add new ModelRemain fields (no behavior change)

**Files:**
- Modify: `src/providers/minimax.rs` — `ModelRemain` struct inside `parse_response` (around line 100)
- Test: existing inline tests in `src/providers/minimax.rs::tests`

**Interfaces:**
- Consumes: existing `parse_response` callers (unchanged)
- Produces: a `ModelRemain` struct that deserializes the four new fields without affecting existing logic.

- [ ] **Step 1: Write a failing test that just deserializes a body containing the new fields**

Add this test inside `#[cfg(test)] mod tests` in `src/providers/minimax.rs`:

```rust
#[test]
fn parses_new_percent_and_status_fields() {
    let body = serde_json::json!({
        "base_resp": {"status_code": 0, "status_msg": ""},
        "model_remains": [{
            "model_name": "general",
            "start_time": 0, "end_time": 1, "remains_time": 0,
            "current_interval_total_count": 0,
            "current_interval_usage_count": 0,
            "current_interval_remaining_percent": 99,
            "current_interval_status": 1,
            "current_weekly_total_count": 0,
            "current_weekly_usage_count": 0,
            "current_weekly_remaining_percent": 98,
            "current_weekly_status": 1,
            "weekly_end_time": 0
        }]
    });
    // No assertion yet — just ensure it deserializes without panicking.
    let _ = parse_response(&body).unwrap();
}
```

- [ ] **Step 2: Run the test — expect failure**

```bash
cd .worktrees/minimax-payload-update && cargo test -j 2 providers::minimax::tests::parses_new_percent_and_status_fields
```

Expected: FAIL with `unknown field ... remaining_percent` or similar Serde error.

- [ ] **Step 3: Add the four fields to ModelRemain**

Edit `src/providers/minimax.rs`, inside the `parse_response` function, the `ModelRemain` struct. Add directly after `weekly_remains_time`:

```rust
        #[serde(default)]
        weekly_remains_time: i64,
        #[serde(default)]
        current_interval_remaining_percent: Option<u8>,
        #[serde(default)]
        current_interval_status: Option<i32>,
        #[serde(default)]
        current_weekly_remaining_percent: Option<u8>,
        #[serde(default)]
        current_weekly_status: Option<i32>,
```

- [ ] **Step 4: Run the test — expect pass**

```bash
cd .worktrees/minimax-payload-update && cargo test -j 2 providers::minimax::tests
```

Expected: PASS, including the existing `parses_minimax_remains_payload` and `coding_plan_model_sorts_first` tests (no regressions).

- [ ] **Step 5: Lint**

```bash
cd .worktrees/minimax-payload-update && cargo clippy -- -D warnings -j 2
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
cd .worktrees/minimax-payload-update && git add src/providers/minimax.rs && git commit -m "minimax: deserialize new *_remaining_percent and *_status fields"
```

---

## Task 3: Percent-derived window emission (TDD)

**Files:**
- Modify: `src/providers/minimax.rs` — `parse_response` body, in the per-model interval/weekly emission blocks

**Interfaces:**
- Consumes: existing `ModelRemain` (now including percent/status fields)
- Produces: a `QuotaWindow` derived from `*_remaining_percent` when `total_count == 0`. Labels remain `5h/<short>` and `wk/<short>`.

- [ ] **Step 1: Write failing test for percent-only model**

Replace `parses_new_percent_and_status_fields` with these two tests (keep the existing legacy tests):

```rust
#[test]
fn percent_only_model_emits_two_windows() {
    let body = serde_json::json!({
        "base_resp": {"status_code": 0, "status_msg": ""},
        "model_remains": [{
            "model_name": "general",
            "start_time": 0, "end_time": 1, "remains_time": 0,
            "current_interval_total_count": 0,
            "current_interval_usage_count": 0,
            "current_interval_remaining_percent": 99,
            "current_interval_status": 1,
            "current_weekly_total_count": 0,
            "current_weekly_usage_count": 0,
            "current_weekly_remaining_percent": 98,
            "current_weekly_status": 1,
            "weekly_end_time": 0
        }]
    });
    let quota = parse_response(&body).unwrap();
    assert_eq!(quota.windows.len(), 2);
    let five = quota.windows.iter().find(|w| w.window_type.starts_with("5h")).expect("5h window");
    assert_eq!(five.limit, 100);
    assert_eq!(five.remaining, 99);
    assert_eq!(five.used, 1);
    let weekly = quota.windows.iter().find(|w| w.window_type.starts_with("wk")).expect("wk window");
    assert_eq!(weekly.limit, 100);
    assert_eq!(weekly.remaining, 98);
    assert_eq!(weekly.used, 2);
}
```

- [ ] **Step 2: Run test — expect failure**

```bash
cd .worktrees/minimax-payload-update && cargo test -j 2 providers::minimax::tests::percent_only_model_emits_two_windows
```

Expected: FAIL (currently zero windows emitted because `total_count == 0`).

- [ ] **Step 3: Implement three-tier logic in parse_response**

Inside the existing `if m.total_count > 0 { ... }` block for the interval window, the new logic must remain **backward compatible**. Replace the existing block with:

```rust
        if m.total_count > 0 {
            let limit = m.total_count;
            let remaining = m.usage_count.clamp(0, limit);
            let used = limit - remaining;
            let label = format!("5h/{}", short_model_name(&m.model_name));
            let period_seconds = if m.end_time > m.start_time && m.start_time > 0 {
                Some((m.end_time - m.start_time) / 1000)
            } else {
                Some(18000)
            };
            windows.push(QuotaWindow {
                window_type: label,
                used,
                limit,
                remaining,
                reset_at: Utc.timestamp_millis_opt(m.end_time).single(),
                period_seconds,
            });
        } else if let Some(pct) = m.current_interval_remaining_percent {
            let pct = pct.min(100);
            let limit = 100;
            let remaining = pct;
            let used = (100 - pct).clamp(0, 100);
            let label = format!("5h/{}", short_model_name(&m.model_name));
            windows.push(QuotaWindow {
                window_type: label,
                used,
                limit,
                remaining,
                reset_at: Utc.timestamp_millis_opt(m.end_time).single(),
                period_seconds: Some(18000),
            });
        }
```

Apply the analogous change for the weekly block — replace its `if m.weekly_total > 0 { ... }` with a count-or-percent branch that uses `m.current_weekly_remaining_percent` as the fallback and default period `7 * 86400`.

- [ ] **Step 4: Run tests — expect pass**

```bash
cd .worktrees/minimax-payload-update && cargo test -j 2 providers::minimax::tests
```

Expected: PASS, including legacy `parses_minimax_remains_payload` (count path unchanged) and the new `percent_only_model_emits_two_windows`.

- [ ] **Step 5: Lint**

```bash
cd .worktrees/minimax-payload-update && cargo clippy -- -D warnings -j 2
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
cd .worktrees/minimax-payload-update && git add src/providers/minimax.rs && git commit -m "minimax: derive interval/weekly windows from *_remaining_percent when total_count is 0"
```

---

## Task 4: Static plan_name fallback (TDD)

**Files:**
- Modify: `src/providers/minimax.rs` — `parse_response`, where `plan_name` is assigned

**Interfaces:**
- Consumes: ordered `model_remains` and existing `is_coding` matcher
- Produces: `plan_name = "MiniMax · MiniMax Coding Plan"` when no model matches the `minimax-m*` / `coding-plan*` priority rules, regardless of whether other models exist.

- [ ] **Step 1: Write failing test for fallback**

Add inside `mod tests`:

```rust
#[test]
fn unknown_models_fall_back_to_static_plan_name() {
    let body = serde_json::json!({
        "base_resp": {"status_code": 0, "status_msg": ""},
        "model_remains": [{
            "model_name": "image-01",
            "start_time": 0, "end_time": 1, "remains_time": 0,
            "current_interval_total_count": 10,
            "current_interval_usage_count": 3,
            "current_interval_remaining_percent": 100,
            "current_interval_status": 1,
            "current_weekly_total_count": 0,
            "current_weekly_usage_count": 0,
            "current_weekly_remaining_percent": 100,
            "current_weekly_status": 1,
            "weekly_end_time": 0
        }]
    });
    let quota = parse_response(&body).unwrap();
    assert_eq!(quota.plan_name, "MiniMax · MiniMax Coding Plan");
}
```

- [ ] **Step 2: Run test — expect failure**

```bash
cd .worktrees/minimax-payload-update && cargo test -j 2 providers::minimax::tests::unknown_models_fall_back_to_static_plan_name
```

Expected: FAIL with message containing `"MiniMax · image-01"`.

- [ ] **Step 3: Update plan_name logic**

In `parse_response`, replace the `let plan_name = ...` block (currently ~10 lines, uses `unwrap_or_else(|| "MiniMax Coding Plan".to_string())` only when there are no models) with:

```rust
    let plan_name = resp
        .model_remains
        .iter()
        .find(|m| is_coding(&m.model_name))
        .map(|m| m.model_name.clone())
        .unwrap_or_else(|| "MiniMax Coding Plan".to_string());
```

(The existing single fallback already says `"MiniMax Coding Plan"` — the prior code re-used the first model's name when no coding-plan model was found. The change is to drop the `or_else(|resp| resp.model_remains.first()...)` arm.)

- [ ] **Step 4: Run tests — expect pass and no regressions**

```bash
cd .worktrees/minimax-payload-update && cargo test -j 2 providers::minimax::tests
```

Expected: PASS for all four tests: existing `parses_minimax_remains_payload`, `coding_plan_model_sorts_first`, new `percent_only_model_emits_two_windows`, new `unknown_models_fall_back_to_static_plan_name`.

- [ ] **Step 5: Lint**

```bash
cd .worktrees/minimax-payload-update && cargo clippy -- -D warnings -j 2
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
cd .worktrees/minimax-payload-update && git add src/providers/minimax.rs && git commit -m "minimax: plan_name falls back to 'MiniMax Coding Plan' when no coding model present"
```

---

## Task 5: Add live fixture file

**Files:**
- Create: `tests/fixtures/minimax/coding_plan_remains_live.json`

**Interfaces:** none — pure data.

- [ ] **Step 1: Write the JSON file**

Write to `tests/fixtures/minimax/coding_plan_remains_live.json`:

```json
{
  "base_resp": {"status_code": 0, "status_msg": ""},
  "model_remains": [
    {
      "model_name": "general",
      "start_time": 1783746000000,
      "end_time": 1783764000000,
      "remains_time": 18000000,
      "current_interval_total_count": 0,
      "current_interval_usage_count": 0,
      "current_interval_remaining_percent": 99,
      "current_interval_status": 1,
      "current_weekly_total_count": 0,
      "current_weekly_usage_count": 0,
      "current_weekly_remaining_percent": 98,
      "current_weekly_status": 1,
      "weekly_start_time": 1783296000000,
      "weekly_end_time": 1783900800000,
      "weekly_remains_time": 139910874
    },
    {
      "model_name": "video",
      "start_time": 1783728000000,
      "end_time": 1783814400000,
      "remains_time": 86400000,
      "current_interval_total_count": 3,
      "current_interval_usage_count": 3,
      "current_interval_remaining_percent": 100,
      "current_interval_status": 1,
      "current_weekly_total_count": 21,
      "current_weekly_usage_count": 21,
      "current_weekly_remaining_percent": 100,
      "current_weekly_status": 1,
      "weekly_start_time": 1783296000000,
      "weekly_end_time": 1783900800000,
      "weekly_remains_time": 139910874
    }
  ]
}
```

These millisecond timestamps are anchored at illustrative values (relative to `2026-07-11 09:00 UTC`). The next task's test re-anchors them with `chrono::Utc::now()` so they don't rot.

- [ ] **Step 2: Verify file parses as JSON**

```bash
cd .worktrees/minimax-payload-update && python3 -c "import json,sys; json.load(open('tests/fixtures/minimax/coding_plan_remains_live.json'))" && echo OK
```

Expected: `OK`.

- [ ] **Step 3: Commit**

```bash
cd .worktrees/minimax-payload-update && git add tests/fixtures/minimax/coding_plan_remains_live.json && git commit -m "test: live MiniMax coding_plan_remains fixture (general + video)"
```

---

## Task 6: Add fixture-driven live-fixture test

**Files:**
- Modify: `src/providers/minimax.rs` — add `fixture()` helper in `mod tests` and a new test

**Interfaces:**
- Consumes: `tests/fixtures/minimax/coding_plan_remains_live.json`
- Produces: a passing `parses_live_fixture_mixed_count_and_percent` test

- [ ] **Step 1: Write failing test**

Add to `mod tests` in `src/providers/minimax.rs`, near the top of the mod:

```rust
    fn fixture(name: &str) -> serde_json::Value {
        use std::path::PathBuf;
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/minimax")
            .join(name);
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read fixture {}: {}", path.display(), e));
        serde_json::from_str(&raw).expect("parse fixture json")
    }
```

Then add the test:

```rust
    #[test]
    fn parses_live_fixture_mixed_count_and_percent() {
        let now = chrono::Utc::now().timestamp_millis();
        let mut body = fixture("coding_plan_remains_live.json");
        for m in body["model_remains"].as_array_mut().unwrap() {
            m["start_time"] = serde_json::json!(now - 3_600_000);          // 1h ago
            m["end_time"] = serde_json::json!(now + 4 * 3_600_000);       // +4h
            m["weekly_start_time"] = serde_json::json!(now - 6 * 86_400_000); // -6d
            m["weekly_end_time"] = serde_json::json!(now + 1 * 86_400_000);  // +1d
        }
        let quota = parse_response(&body).unwrap();
        assert_eq!(quota.plan_name, "MiniMax · MiniMax Coding Plan");
        assert_eq!(quota.windows.len(), 4);

        let five_g = quota.windows.iter().find(|w| w.window_type == "5h/general").expect("5h/general");
        assert_eq!(five_g.limit, 100);
        assert_eq!(five_g.remaining, 99);
        assert_eq!(five_g.used, 1);

        let wk_g = quota.windows.iter().find(|w| w.window_type == "wk/general").expect("wk/general");
        assert_eq!(wk_g.limit, 100);
        assert_eq!(wk_g.remaining, 98);
        assert_eq!(wk_g.used, 2);

        let five_v = quota.windows.iter().find(|w| w.window_type == "5h/video").expect("5h/video");
        assert_eq!(five_v.limit, 3);
        assert_eq!(five_v.remaining, 3);
        assert_eq!(five_v.used, 0);

        let wk_v = quota.windows.iter().find(|w| w.window_type == "wk/video").expect("wk/video");
        assert_eq!(wk_v.limit, 21);
        assert_eq!(wk_v.remaining, 21);
        assert_eq!(wk_v.used, 0);
    }
```

- [ ] **Step 2: Run test — expect failure (fixture path missing implies PANIC, which is failing test)**

```bash
cd .worktrees/minimax-payload-update && cargo test -j 2 providers::minimax::tests::parses_live_fixture_mixed_count_and_percent
```

Expected: PASS — assuming Tasks 2–4 already succeeded.

If PASS already, no change needed; if FAIL, debug by reading the assertion message and adjust either the parser or the test (do not silently adjust the spec math).

- [ ] **Step 3: Lint and full-suite test**

```bash
cd .worktrees/minimax-payload-update && cargo test -j 2 && cargo clippy -- -D warnings -j 2
```

Expected: all tests PASS, clippy clean.

- [ ] **Step 4: Commit**

```bash
cd .worktrees/minimax-payload-update && git add src/providers/minimax.rs && git commit -m "test: parses_live_fixture_mixed_count_and_percent covers general+video live shape"
```

---

## Task 7: Add depleted-status always-render test (TDD for parity)

**Files:**
- Modify: `src/providers/minimax.rs::tests`

**Interfaces:** none.

- [ ] **Step 1: Write the test**

```rust
    #[test]
    fn depleted_window_status_zero_still_renders() {
        let body = serde_json::json!({
            "base_resp": {"status_code": 0, "status_msg": ""},
            "model_remains": [{
                "model_name": "general",
                "start_time": 0, "end_time": 1, "remains_time": 0,
                "current_interval_total_count": 0,
                "current_interval_usage_count": 0,
                "current_interval_remaining_percent": 0,
                "current_interval_status": 0,
                "current_weekly_total_count": 0,
                "current_weekly_usage_count": 0,
                "current_weekly_remaining_percent": 0,
                "current_weekly_status": 0,
                "weekly_end_time": 0
            }]
        });
        let quota = parse_response(&body).unwrap();
        let five = quota.windows.iter().find(|w| w.window_type.starts_with("5h")).expect("5h window");
        assert_eq!(five.limit, 100);
        assert_eq!(five.remaining, 0);
        assert_eq!(five.used, 100);
    }
```

- [ ] **Step 2: Run test — expect pass (Task 3 already implements this)**

```bash
cd .worktrees/minimax-payload-update && cargo test -j 2 providers::minimax::tests::depleted_window_status_zero_still_renders
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
cd .worktrees/minimax-payload-update && git add src/providers/minimax.rs && git commit -m "test: depleted interval (status=0, pct=0) still renders as 100% used"
```

---

## Task 8: Drop MiniMax hack from dashboard.rs

**Files:**
- Modify: `src/tui/dashboard.rs` — five surgical removals (see spec §TUI changes)

**Interfaces:**
- Consumes: the existing generic-window rendering path
- Produces: a MiniMax card that renders via the same code path as every other provider

Sub-tasks ordered from most local to most visible so each commit keeps the build green.

### Task 8a: Remove `render_minimax_windows` and `minimax_bar_cell`

- [ ] **Step 1: Find the two functions**

```bash
cd .worktrees/minimax-payload-update && grep -n '^fn render_minimax_windows\|^fn minimax_bar_cell' src/tui/dashboard.rs
```

Expected: two line numbers (render_minimax_windows higher up).

- [ ] **Step 2: Delete both function definitions in one edit**

Open `src/tui/dashboard.rs`, delete the entire bodies of `render_minimax_windows` and `minimax_bar_cell` (the function including the comment block above `render_minimax_windows`). Confirm the file compiles cleanly:

```bash
cd .worktrees/minimax-payload-update && cargo build -j 2 2>&1 | tail -20
```

Expected: FAIL with "undefined references" because callers still expect the deleted functions. That's expected here — proceed to 8b.

### Task 8b: Remove the inline caller in `render_entry`

- [ ] **Step 1: Delete the inline branch**

In `render_entry`, locate:

```rust
                if result.kind == ProviderKind::Minimax {
                    render_minimax_windows(&mut lines, &quota.windows, inner.width);
                    let paragraph = Paragraph::new(Text::from(lines))
                        .alignment(ratatui::layout::Alignment::Left);
                    f.render_widget(paragraph, inner);
                    return;
                }
```

Delete it (the surrounding code falls through to the generic renderer with no other change).

- [ ] **Step 2: Build — expect green**

```bash
cd .worktrees/minimax-payload-update && cargo build -j 2 2>&1 | tail -20
```

Expected: `Finished` with no errors.

- [ ] **Step 3: Run full tests**

```bash
cd .worktrees/minimax-payload-update && cargo test -j 2
```

Expected: all green.

- [ ] **Step 4: Commit both 8a + 8b together**

```bash
cd .worktrees/minimax-payload-update && git add src/tui/dashboard.rs && git commit -m "tui: drop MiniMax-specific paired window renderer"
```

### Task 8c: Remove the 2-col span / weight / height MiniMax branches

- [ ] **Step 1: Remove `is_minimax` branch in `flow_placements`**

In `flow_placements`, delete these lines:

```rust
            let is_minimax = matches!(&entries[entry_idx],
                ProviderEntry::Done(r) | ProviderEntry::Refreshing(r)
                    if r.kind == ProviderKind::Minimax);

            let span = if is_minimax { 2 } else { 1 }.min(cols);
            let row_span = if is_minimax && allow_spanning { 2 } else { 1 };
```

Replace with:

```rust
            let span = 1usize.min(cols);
            let row_span = 1usize;
```

- [ ] **Step 2: Remove the `r.kind == ProviderKind::Minimax` arm in `natural_card_height`**

Find the `if r.kind == ProviderKind::Minimax` arm in `natural_card_height`. The whole `let content = if ... { ... } else { ... };` collapses to just the `else` branch:

```rust
                    let content: u16 = (visible as u16) * 2 + 1;
```

- [ ] **Step 3: Remove the `r.kind == ProviderKind::Minimax` arm in `card_weight`**

Replace the `let effective = if r.kind == ProviderKind::Minimax { ... } else { visible };` line with:

```rust
                    let effective = visible;
```

- [ ] **Step 4: Build and test**

```bash
cd .worktrees/minimax-payload-update && cargo build -j 2 && cargo test -j 2 && cargo clippy -- -D warnings -j 2
```

Expected: clean across all three.

- [ ] **Step 5: Commit**

```bash
cd .worktrees/minimax-payload-update && git add src/tui/dashboard.rs && git commit -m "tui: drop MiniMax column-span, height, and weight special-cases"
```

---

## Task 9: Remove `vertical_spanning` from config and CLI

**Files:**
- Modify: `src/config.rs` — remove `pub vertical_spanning: bool` from `UIConfig`
- Modify: `src/main.rs` — remove the `vertical_spanning` CLI flag and the `dashboard.vertical_spanning = config.ui.vertical_spanning;` line

**Interfaces:** none — feature flag fully removed; default behavior is 1-col always.

- [ ] **Step 1: Locate all references**

```bash
cd .worktrees/minimax-payload-update && grep -rn 'vertical_spanning' src/
```

Expected: matches in `src/config.rs`, `src/main.rs`, `src/tui/dashboard.rs` (you already removed the `dashboard.vertical_spanning = ...` reader side if it sat in `dashboard.rs` — otherwise it lives in `main.rs`).

- [ ] **Step 2: Edit `src/config.rs`**

In the `UIConfig` struct (around line 41), delete the `pub vertical_spanning: bool,` line. If there is a literal default in the same struct's impl, also remove the `vertical_spanning: false` line(s).

- [ ] **Step 3: Edit `src/main.rs`**

- Delete the `--ui-vertical-spanning` arg (or equivalent) from the CLI definitions and the matching `matches.value_of("vertical_spanning")` block.
- Delete the `dashboard.vertical_spanning = config.ui.vertical_spanning;` line (around line 902).

- [ ] **Step 4: Build, test, lint**

```bash
cd .worktrees/minimax-payload-update && cargo build -j 2 && cargo test -j 2 && cargo clippy -- -D warnings -j 2
```

Expected: clean.

- [ ] **Step 5: Commit**

```bash
cd .worktrees/minimax-payload-update && git add src/config.rs src/main.rs && git commit -m "ui: drop vertical_spanning config (no longer needed)"
```

---

## Task 10: Update CLAUDE.md

**Files:**
- Modify: `CLAUDE.md` — remove `vertical_spanning = true` example line and surrounding text about MiniMax-as-2×2

**Interfaces:** none.

- [ ] **Step 1: Locate the relevant block**

```bash
cd .worktrees/minimax-payload-update && grep -n 'vertical_spanning\|vertical spanning\|2x2\|MiniMax as' CLAUDE.md
```

Expected: a small block of one to three lines mentioning the feature.

- [ ] **Step 2: Delete the `vertical_spanning = true` line and reword the surrounding sentence**

Replace the surrounding sentence (something like "experimental: MiniMax as 2x2 card") with a tighter version, e.g., remove the line entirely if the comment immediately above was solely about MiniMax-as-2×2.

- [ ] **Step 3: Verify no other references**

```bash
cd .worktrees/minimax-payload-update && grep -n 'vertical_spanning\|2x2\|MiniMax as' CLAUDE.md README.md
```

Expected: no matches.

- [ ] **Step 4: Commit**

```bash
cd .worktrees/minimax-payload-update && git add CLAUDE.md && git commit -m "docs: drop obsolete vertical_spanning config note from CLAUDE.md"
```

---

## Task 11: After-snap + multi-size screenshots + log.md entry

**Files:**
- Create: `screenshots/after-bpayload.txt`
- Create: `screenshots/snap-{80x20,80x30,120x40,160x50,200x60}.{txt,ansi}` (via `just screenshots-multi`)
- Modify: `screenshots/log.md`

**Interfaces:** none.

- [ ] **Step 1: After-snap at 160x50**

```bash
cd /home/xertrov/src/quotas/.worktrees/minimax-payload-update && \
  cargo run -- --snap --snap-width 160 --snap-height 50 \
    --snap-output ../../screenshots/after-bpayload.txt
```

Expected: file exists.

- [ ] **Step 2: Visual diff at 160x50**

```bash
diff -u /home/xertrov/src/quotas/screenshots/before-bpayload.txt /home/xertrov/src/quotas/screenshots/after-bpayload.txt | head -80
```

Expected: MiniMax card now occupies 1 column instead of 2, and shows multiple bar rows (general + video × 2 windows each) instead of paired mini-bars.

- [ ] **Step 3: Run the full multi-size capture (requires tmux)**

```bash
cd /home/xertrov/src/quotas/.worktrees/minimax-payload-update && just screenshots-multi
```

Expected: 5×2 files written to `screenshots/`.

- [ ] **Step 4: Write log.md entry**

Append to `screenshots/log.md`:

```markdown
## Iter N — MiniMax payload update

- MiniMax card dropped from 2-col + paired layout to standard 1-col, generic window rendering.
- MiniMax windows now show: 5h/general, wk/general (derived from percent), 5h/video, wk/video (derived from counts).
- All 5 sizes re-snapshotted (`screenshots/snap-*.txt` and `screenshots/snap-*.ansi`).
- Visual diff: MiniMax card narrower and taller (4 windows stacked). No neighboring card regressions.
```

Replace `N` with the next iter number per existing entries in the file.

- [ ] **Step 5: Commit screenshots and log**

```bash
cd .worktrees/minimax-payload-update && git add screenshots/ && git commit -m "screenshots: refresh for MiniMax payload update (5 sizes + log entry)"
```

---

## Task 12: Final verification

**Files:** none — verification only.

- [ ] **Step 1: Run the full test suite**

```bash
cd .worktrees/minimax-payload-update && cargo test -j 2
```

Expected: all tests pass.

- [ ] **Step 2: Lint**

```bash
cd .worktrees/minimax-payload-update && cargo clippy -- -D warnings -j 2
```

Expected: clean.

- [ ] **Step 3: Release build**

```bash
cd .worktrees/minimax-payload-update && cargo build --release -j 2
```

Expected: `Finished release`.

- [ ] **Step 4: Final commit if any cleanup needed**

If the previous steps revealed a stray comment or unused `use ProviderKind::Minimax;` import left behind after removing the inline branch:

```bash
cd .worktrees/minimax-payload-update && cargo build -j 2 2>&1 | grep 'warning\|unused'
```

Address each warning inline, then:

```bash
cd .worktrees/minimax-payload-update && git add -A && git commit -m "chore: final cleanup after MiniMax payload update"
```

(Only commit if there were changes.)

- [ ] **Step 5: Merge back to master (per Max's repo norms)**

From the parent repo (not the worktree):

```bash
cd /home/xertrov/src/quotas && git merge --no-ff minimax-payload-update -m "Merge minimax-payload-update: refresh MiniMax parser, fixtures, TUI"
```

Then clean up the worktree:

```bash
cd /home/xertrov/src/quotas && git worktree remove .worktrees/minimax-payload-update --force
```

Final check: `git status --porcelain --ignored` — if anything important appears in the worktree path, move it into the main checkout before the `--force` removal.
