# Detail Favorites Refresh Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add compact/normal detail layouts, persisted provider/quota favorites and hidden quotas, detail-only auto-refresh scoping, and ship the release.

**Architecture:** Extend config with persisted UI preferences, teach dashboard/detail state about row focus and display mode, keep detail rendering in `src/tui/detail.rs`, and gate refresh scheduling in `src/main.rs` based on whether the user is in dashboard or detail view.

**Tech Stack:** Rust, ratatui, crossterm, serde/toml, cargo test, tmux screenshot helpers

---

### Task 1: Detail layout modes and header lift

**Files:**
- Modify: `src/tui/detail.rs`
- Modify: `src/tui/dashboard.rs`
- Modify: `src/main.rs`
- Test: `src/tui/detail.rs`
- Verify artifacts: `screenshots/`

- [ ] **Step 1: Add failing tests for compact-mode selection and header metadata placement**

Add tests in `src/tui/detail.rs` covering:

```rust
#[test]
fn compact_mode_moves_plan_and_freshness_into_header() {
    let out = render_detail_text(gemini_fraction_quota_result(), 80, 18);
    assert!(out.contains("Gemini API"));
    assert!(out.contains("Updated"));
}

#[test]
fn compact_mode_renders_denser_rows_on_small_widths() {
    let out = render_detail_text(gemini_fraction_quota_result(), 80, 12);
    assert!(out.contains("4% used"));
}
```

- [ ] **Step 2: Run the detail tests to confirm the new expectations fail**

Run: `cargo test src::tui::detail -- --nocapture`
Expected: FAIL because the current renderer does not have compact mode or lifted header metadata.

- [ ] **Step 3: Implement detail-mode state and richer detail rendering**

Add:

- a resolved detail display mode (`Auto`, `Normal`, `Compact`) owned by dashboard state;
- header rendering that surfaces provider, plan, freshness, and refresh-progress above the fold;
- compact row rendering in `src/tui/detail.rs`.

- [ ] **Step 4: Wire keyboard toggle and detail hint text**

Update `src/main.rs` and `src/tui/dashboard.rs` so `Tab` cycles the mode override while detail is open and the detail header mentions the control.

- [ ] **Step 5: Run focused tests and screenshot iteration**

Run:
- `cargo test src::tui::detail -- --nocapture`
- `cargo build -q`
- `cargo run -- --snap --snap-width 120 --snap-height 40 --snap-output screenshots/iter9.txt`
- `cargo run -- --snap --snap-width 80 --snap-height 20 --snap-output screenshots/iter10.txt`

Expected: PASS on tests and visibly improved detail layout at narrow sizes.

- [ ] **Step 6: Commit**

```bash
git add src/tui/detail.rs src/tui/dashboard.rs src/main.rs screenshots/iter9.txt screenshots/iter10.txt
git commit -m "feat: add compact detail view modes"
```

### Task 2: Persisted favorites and hidden quotas

**Files:**
- Modify: `src/config.rs`
- Modify: `src/tui/dashboard.rs`
- Modify: `src/tui/detail.rs`
- Modify: `src/main.rs`
- Possibly modify: `README.md`
- Test: `src/config.rs`, `src/tui/detail.rs`, `src/tui/dashboard.rs`

- [ ] **Step 1: Add failing config and ordering tests**

Add tests for:

- provider favorites parsing;
- per-provider quota favorites/hidden parsing;
- round-tripping serialized config;
- dashboard visual order preferring favorited providers;
- detail ordering preferring favorited quotas and rendering hidden rows dimmed.

- [ ] **Step 2: Run targeted tests and confirm failure**

Run: `cargo test config tui::detail tui::dashboard -- --nocapture`
Expected: FAIL because preferences and ordering do not exist yet.

- [ ] **Step 3: Implement persisted preference structs and config writes**

Add typed config sections for:

```rust
pub struct FavoritesConfig {
    pub providers: Vec<String>,
}

pub struct QuotaPreferences {
    pub favorites: Vec<String>,
    pub hidden: Vec<String>,
}
```

Also add helper methods to:

- query whether a provider/quota is favorited/hidden;
- toggle those values;
- write updated config to `~/.config/quotas/config.toml`.

- [ ] **Step 4: Implement dashboard/detail interactions**

Add:

- provider favorite marker and sort priority on dashboard;
- provider favorite toggle on `f` from dashboard;
- detail row focus;
- quota favorite/hide toggles on `f` and `x`;
- dimmed hidden-row controls for unhiding;
- favorite markers in detail header and rows.

- [ ] **Step 5: Run focused tests and update docs**

Run:
- `cargo test config tui::detail tui::dashboard -- --nocapture`
- `cargo build -q`

Update `README.md` with the new config section and key bindings.

- [ ] **Step 6: Commit**

```bash
git add src/config.rs src/tui/dashboard.rs src/tui/detail.rs src/main.rs README.md
git commit -m "feat: add persisted favorites and hidden quotas"
```

### Task 3: Detail-only auto-refresh scoping

**Files:**
- Modify: `src/main.rs`
- Modify: `src/tui/dashboard.rs`
- Test: `src/main.rs` or extracted helper tests

- [ ] **Step 1: Add failing refresh-gating tests**

Extract refresh-eligibility logic into helper functions and add tests for:

- dashboard view refreshes all eligible providers on their cadence;
- detail view refreshes only the selected provider;
- manual refresh still targets all eligible providers.

- [ ] **Step 2: Run the refresh tests to confirm failure**

Run: `cargo test refresh -- --nocapture`
Expected: FAIL because current auto-refresh scans every provider regardless of detail state.

- [ ] **Step 3: Implement scoped refresh scheduling**

Update the scheduler in `src/main.rs` so:

- manual refresh remains global;
- periodic refresh checks `dashboard.show_detail` and only refreshes `dashboard.selected_index` when detail is visible.

- [ ] **Step 4: Run focused verification**

Run:
- `cargo test refresh -- --nocapture`
- `cargo test`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs src/tui/dashboard.rs
git commit -m "fix: scope detail auto-refresh to visible provider"
```

### Task 4: Final TUI polish and screenshot pass

**Files:**
- Modify: `screenshots/log.md`
- Update: `screenshots/*.txt`, `screenshots/*.ansi` as needed

- [ ] **Step 1: Build and capture final snapshots**

Run:
- `cargo build -q`
- `just screenshots-multi`
- `just screenshot-detail 160 50 screenshots/detail-160x50.txt`
- `just screenshot-detail 80 20 screenshots/detail-80x20.txt`

Expected: updated snapshots across standard sizes.

- [ ] **Step 2: Review the narrow renders and log observations**

Record any final layout adjustments in `screenshots/log.md`, then make the smallest required polish edits.

- [ ] **Step 3: Re-run verification**

Run:
- `cargo test`
- `cargo clippy -- -D warnings`

- [ ] **Step 4: Commit**

```bash
git add screenshots src/tui/detail.rs src/tui/dashboard.rs screenshots/log.md
git commit -m "test: refresh TUI screenshots for detail view changes"
```

### Task 5: Release prep and publish

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Create or modify: `CHANGELOG.md`

- [ ] **Step 1: Add release notes**

Create or update `CHANGELOG.md` with the next release entry summarizing:

- compact detail mode;
- header freshness/subscription improvements;
- provider/quota favorites;
- hidden quota controls;
- detail-only auto-refresh scoping.

- [ ] **Step 2: Bump version**

Update `Cargo.toml` and `Cargo.lock` to the next version.

- [ ] **Step 3: Run release verification**

Run:
- `cargo test`
- `cargo clippy -- -D warnings`
- `cargo build --release`

- [ ] **Step 4: Commit release prep**

```bash
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "chore: prepare vNEXT release"
```

- [ ] **Step 5: Tag, push, release, publish**

Run:
- `git tag vNEXT`
- `git push origin HEAD`
- `git push origin vNEXT`
- create or verify GitHub release with changelog and release artifacts
- `cargo publish`

Expected: pushed tag, published crate, and released artifacts.
