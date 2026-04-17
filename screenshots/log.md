# Design iteration log

Each entry is one round of screenshot-then-improve. Files live next to this
one: `iterN.txt` (plain) and `iterN.ansi` (ANSI colors).

---

## Iter 1 — baseline after minimax inversion fix

**What shipped to this capture**
- Minimax `current_interval_usage_count` / `current_weekly_usage_count`
  treated as remaining (API naming quirk, verified against openclaw
  minimax-usage.sh). Before: minimax showed 98%/99% everywhere. After:
  coding plans ~3%, most models 0%, speech-hd 39%.
- Stale footer legend replaced (`▒ ahead  █ overspend`) from last round.
- `5h/` / `wk/` prefixes stripped under section headers (last round).

**Observations**
1. Minimax values now look correct and match user expectation.
2. **Row-width overflow bug**: minimax OneLine mode wraps
   `147.8k` onto a second line for wk/coding-plan-vlm (and M*). The
   label + bar + % + number exceeds the 47-char card width. Need to either
   widen label or trim number format or narrow bar.
3. `coding-pla…` still collides between vlm/search variants (unchanged —
   suffix-preserving trunc task covers this).
4. `monthly_mcp`, `weekly_sonn…`, `extra_credi…` — claude's window types
   are ugly. Labels module should rename these for display.
5. Minimax 7d rows have the number wrapping issue *and* the bars are
   all ~1 cell — the week is very early, so everyone's at 0-1%. Bars
   convey little information at this scale.
6. Whitespace: Z.ai has 2 windows but the card is 22 rows tall.
   Row height adapts only to the heaviest card in the row.

**Ideas queued for iter 2 (from user's latest message)**
- [x] Bar overlay text: `X% (used/cap)` drawn inside the bar using cell
  backgrounds.
- [ ] Single-card rows expand to full row width (helps minimax).
- [x] Normalize number format to "used" direction everywhere.
- [ ] Suffix-preserving truncation (`…lan-vlm` vs `…-search`).
- [ ] Provider-aware label renames (claude `weekly` → `7d`, etc.).
- [ ] Custom minimax 2-col (5h | 7d per model).

---

## Iter 2 — bar overlay text working

**Shipped**
- New `bar::build_labeled(used_pct, time_elapsed, width, color, overlay)`.
  Text is centered in the bar. Cells under the overlay adopt a
  cell-type-aware style: filled cells → bg=base color, fg=black bold;
  overspend → bg=orange, fg=black bold; slack → bg=DarkGray, fg=white
  bold; future → no bg, fg=white bold.
- Dashboard rewritten to call `build_labeled` with
  `"{pct}% ({used}/{limit})"`. Second row in TwoLine mode shows
  `resets in Xh Ym` instead of the old `{used}/{cap} left` line.
- Label column widened to 12 chars for both modes; bar width now extends
  almost to the card edge (no reserved trailing chars for "% left").

**Observations**
1. Big visual win: values are legible *inside* the bar, no more wrapping.
2. **Text bg varies mid-string** — when "82% (82/100)" sits partly over
   green and partly over orange cells, each character takes the bg of the
   cell underneath. It does look "embossed" and it surfaces pace boundary
   info, but it's busier than I'd like. Options for iter 3:
   (a) reserve a uniform bg for the overlay span (one color strip);
   (b) keep the current multi-bg and just rely on it as feature.
   I'll leave (b) for now and see if it grows on me.
3. `coding-plan…` truncation still collides — task #22 still pending.
4. Minimax card still stuck at single-column width — bar maxes at ~32
   chars even though the row has 150+ chars of unused width.
5. Claude `weekly_sonn…` shows ugly label — needs rename map.
6. Edge case: claude `extra_credi…` reads `100% (13.2k/13.0k)`
   (used > limit — credits were top-up, so this is fine data). Bar clamps.
7. Number format on big values (`150.0k`) reads as `150.0k`. Trailing
   `.0` is noise. Fix format_num to drop trailing `.0`.

**Ideas for iter 3**
- [x] Full-row-width when row has fewer cards than grid_columns.
- [x] Suffix-preserving truncation.
- [x] Provider-aware label mapping (claude: weekly→7d, weekly_sonnet→7d Sonnet,
  weekly_opus→7d Opus, monthly_mcp→month mcp, extra_credits→credits).
- [x] Drop trailing `.0` in `format_num`.

---

## Iter 3 — full-row-width + labels + suffix trunc

**Shipped**
- Grid layout: if a row has N<grid_cols cards, the N cards share the
  full row width (was: last row's cards clipped to one col's width).
  Minimax card now spans the full terminal width on row 2.
- `bar::display_label` now applies a small rename table (weekly→7d,
  weekly_sonnet→7d Sonnet, monthly_mcp→month MCP, extra_credits→credits,
  payg_balance→PAYG).
- `bar::truncate_suffix` replaces the prefix-preserving `truncate`.
  `coding-plan-vlm` → `…ng-plan-vlm` rather than `coding-pla…`, so
  vlm/search variants stay distinguishable. Both dashboard and detail
  adopt the new helper.
- `format_num` drops trailing `.0` → `150k` not `150.0k`, `2.5M` stays.
- `_generation`, `Fast-6…` etc. also benefit from the suffix trunc.

**Observations**
1. Minimax now sprawls to full width (~190 chars). That's the right
   direction, but with 22 rows at 3% / 0% the bar is mostly empty and
   the centered "3%" overlay is floating in dead space far from the fill.
   Two improvements worth trying:
   (a) Two-column render: one row per model with 5h and 7d side-by-side
       (cuts rows in half and puts bars at a comparable width to other
       providers).
   (b) Left-align the overlay so it sits near the filled portion of the bar.
2. Long minimax model names (`Hailuo-2.3-Fast-6s-768p`) still truncate
   at 12 chars because label_w is fixed in dashboard. On the full-width
   card label could safely expand to 16-18. Adaptive label_w.
3. Bar overlay text crosses fill/empty boundary and changes styling
   mid-string. Still a bit busy but acceptable.
4. Card heights: top row ~22 lines tall for only ~4 windows of content —
   a lot of whitespace. Weight clamp might be too loose.

**Ideas for iter 4**
- [x] Minimax 2-col render (left=5h, right=7d) keyed by model.
- [ ] Adaptive label_w per card based on widest (post-trunc) label.
- [ ] Reconsider row weights: let top-row (all small cards) get fewer rows.

---

## Iter 4 — minimax 2-col 5h|7d

**Shipped**
- New `render_minimax_windows` helper in `dashboard.rs`: parses each
  window's `5h/` or `wk/` prefix, groups by stripped model name,
  preserves first-sighting order so the minimax `short_model_name`
  ranking (M*, coding-plan-vlm, coding-plan-search first) survives.
- Emits a header row `── 5h ──    ── 7d ──` centered over each bar
  column, then one line per model: `label(20) | 5h bar | 7d bar`.
- Bar width clamps to [10, 90] per side; each bar carries its own
  `X% (used/cap)` overlay via `build_labeled`.
- Dispatch added to `render_done_card`: `result.kind == Minimax` skips
  the generic loop entirely and renders via the helper.
- Missing side (e.g. a model with no 7d pairing) gets a blank filler
  the same width as a bar so the columns stay aligned.

**Observations**
1. Huge readability win: 22 rows → 11 rows, and you can compare 5h to
   7d usage for the same model on one line.
2. **Massive empty space at the bottom** of the minimax card: it's been
   given the tallest row-weight but now only needs ~12 lines. Top row
   cards (zai, kimi, claude, codex) are also mostly empty — they have
   2-4 windows each in cards ~20 rows tall.
3. Row weights need a rethink: minimax should get roughly half the
   weight it used to, or card_weight should account for the 2-col
   compaction. Simpler fix: special-case minimax weight = ~half model
   count, not full window count.
4. Overlay text is still floating in dead space for 0-3% models. A
   left-aligned overlay would hug the filled cells — but for 40%+ bars
   the centered position is fine. Could make alignment adaptive:
   centered if fill >= 25%, else left-anchored to `fill + 1`.
5. Header row `── 5h ──` overlaps the bar region, not great if bars
   touch it. Visual is OK because there's a blank line between header
   and first bar? Actually no — header IS the first line with content
   after the plan name. Reads fine.
6. Long model names (`Hailuo-2.3-Fast-6…`) still clip to 20 chars.
   The label column could widen to 24 on the full-width card.

**Ideas queued for iter 5**
- [x] Shrink `card_weight` for minimax by ~half since 2-col render
  halves vertical footprint. Free up rows for zai/kimi/claude/codex.
- [x] Adaptive overlay alignment: center when fill ≥ 25% else left.
- [ ] Widen minimax label column to 24 chars since the card is wide.

---

## Iter 5 — overlay hugs fill edge + row-weight trim

**Shipped**
- `bar::build_labeled` overlay position now anchors at the fill/empty
  boundary: `overlay_start = (used_cells - overlay_len/2).clamp(...)`.
  - fill ≈ 0 → overlay pinned to left (`0% (0/100)▒▒▒░░░░...`)
  - fill ≈ 50% → centered on the boundary
  - fill ≈ 100% → pinned to right
  - cost: zero; removed the fixed-center logic entirely.
- `card_weight`: for minimax, `effective = visible.div_ceil(2)` since
  the 2-col render halves the vertical footprint of minimax rows.
- Clamp ceiling dropped from 12 → 10 so the minimax trim actually
  translates into row-height redistribution (with ceiling=12 both
  rows clamped identically and the weight change was a no-op).

**Observations**
1. **Overlay edge-anchor is a clear win.** Every low-% bar now has
   text sitting against the fill instead of floating in the middle
   of 80 empty cells. Compare iter4's `3% (405/15k)` stranded at
   center-of-bar to iter5's `3% (405/15k)▒▒▒...` where the `%` digit
   is glued to the filled portion. The text also implicitly encodes
   the fill position as you read down a column.
2. Row weight shift is modest: top row went 22→24 lines, minimax
   row 30→29. Claude/codex cards still have ~14 lines of whitespace
   under their content. The real limiter is that `card_weight` caps
   at 10 and everyone in the top row happens to tie at 8.
3. For the 100%-filled claude `credits` row, overlay now right-pins
   to `100% (13.2k/13k)` — reads naturally as "bar is full, here's
   what full means".
4. `speech-hd` at 42% shows the prettiest case: text sits exactly on
   the green/dark boundary so it reads `████42% (4.6k/11k)░░░░`.
5. Minimax card still has ~15 lines of bottom whitespace. The
   cleanest fix would be a "natural height" mode for cards (render
   content-sized, let the parent absorb slack), but that's a bigger
   refactor than the screenshot round warrants.
6. Top-row cards in TwoLine mode (`resets in Xh Ym` after each bar)
   use the two-line layout fine. With the extra 2 rows this iter
   gained, there's room for pace commentary to appear inline too,
   but keeping the dashboard terse reads better — pace lives in detail.

**Status**
5 iterations complete. Shipping as-is; remaining whitespace is
aesthetic and would need a more invasive layout rework to eliminate.

---

## Iter 6 — natural card heights + minimax reset footer

**Shipped**

---

## Iter 9 — detail layout modes, favorites, and scoped refresh

**Shipped**
- Detail view now resolves between normal and compact layouts automatically, with a manual `Tab` override in-session.
- Provider freshness and plan/subscription metadata moved into the detail header so the first screenful carries the important status.
- Provider favorites now sort first on the dashboard and show a star marker.
- Detail rows support focused quota favorites and hide/unhide actions, with hidden quotas rendered as dim controls instead of disappearing completely.
- Periodic auto-refresh is now scoped to the visible provider while detail view is open.

**Observations**
1. `80x20` no longer wraps quota suffixes in compact mode.
2. The expanded detail hint line is long on narrow terminals, but it still preserves the key actions without corrupting the frame.
3. Dashboard snapshots remain stable after adding favorites because visual order is recomputed intentionally rather than on every refresh.
- `natural_card_height` function: computes the exact vertical footprint
  of a card (borders + header + plan + window rows) so rows use
  `Constraint::Length(natural_h)` instead of `Constraint::Ratio`. A
  trailing `Constraint::Fill(1)` spacer absorbs remaining terminal height.
  Result: top row 22→12 lines, minimax row 30→18 lines. Content is dense;
  slack falls to the bottom of the screen instead of ballooning each card.
- Fixed off-by-one: `natural_card_height` accounts for `footer_reserve=1`
  that `pick_layout` consumes, ensuring Claude/Codex (4 windows) stay in
  TwoLine mode and show `resets in` lines.
- Minimax reset footer row: after all model rows, a single line shows
  `5h resets in Xh Ym` and `7d resets in Nd Xh` centered over the
  respective bar columns. All models share the same window boundary for
  a given period, so one footer covers all. Added `+2` to minimax
  `natural_card_height` to allocate the extra line.
- Dead code from clamp fallback path in grid layout cleaned by clippy fmt.

**Observations**
1. Screen is now dramatically denser — content rows occupy ~30 of 60 lines
   vs 55+ before, with the remaining 25 blank at the bottom of the terminal.
2. Top row cards still show ~2 empty lines inside their border (the row
   height is set by the tallest card — claude needs 13 lines, zai needs
   9, so zai has 4 spare lines inside its box). Acceptable.
3. Minimax reset footer perfectly placed: `5h resets in 3h 1m` and
   `7d resets in 6d 17h` centered over each bar column. Very readable.
4. `credits` row for Claude shows no reset time (no `reset_at` from API)
   — one line instead of two. Minor gap in info.
5. Detail view still has no scroll and no l/r provider-navigation — next.

**Ideas queued for iter 7**
- [x] Detail view: up/down scrolls; l/r navigates providers; Esc/q/Enter returns.
- [x] Detail view: show keyboard hints updated for new shortcuts.
- [ ] Display a "footer" hint bar when content overflows in detail view.

---

## Iter 7 — detail view scroll + provider navigation

**Shipped**
- `detail_scroll: u16` field added to Dashboard; resets to 0 on Enter and
  on any provider-switch.
- Up/Down (and k/j vim keys) scroll the detail view by 3 lines. PageUp/
  PageDown scroll by 20.
- Left/Right (and h/l) navigate prev/next provider in visual order while
  in detail mode (wraps around). Resets scroll to 0 on each jump.
- Header updated: "← → providers  ↑ ↓ scroll  Enter/Esc back  C copy  Q quit"
  visible on line 2, below the title. Fixed a height bug where BOTTOM border
  overwrote the hint (title rect expanded from 2→3 lines).
- Grid view unchanged: arrow keys still navigate the grid as before.
  'h'/'l'/'j'/'k' also work in grid mode for vim-style nav.

**Observations**
1. Detail view now fully scrollable. Raw JSON sections that extend past
   the terminal bottom are reachable.
2. Provider cycling with ← → in detail mode is fast and clean — each
   jump resets scroll so you start at the top of the next provider.
3. The hint text fits comfortably on one line at 200-char width.
4. `q` quits from detail mode (doesn't return to grid) — consistent with
   grid behaviour but might surprise users who expect q to be "go back".
   Esc/Enter/Backspace all return to grid. Acceptable.
5. Bar width in detail view is narrower than grid (capped at 60) so bars
   render more compactly there. Cosmetic only.

**Ideas queued for iter 8**
- [x] Small cards in top row still have 2-4 empty lines at bottom (Z.ai
      has 2 windows but shares row height with Claude's 4 windows).
      Could render a small "quota health" summary in spare space.
- [ ] On grid, 'h'/'l'/'j'/'k' also work — hint text in grid doesn't
      mention them but that's fine; power-user feature.
- [x] Minimax label 20→24, short_model_name truncation removed.

---

## Iter 8 — pacing badge + minimax label width

**Shipped**
- `pace_badge(visible)` helper: scans visible windows for the worst
  pace diff (used_pct - time_elapsed). If diff ≥ 0.08 → orange ⚡
  badge "⚡ worst_label: X% — burning fast". If all windows ≤ -0.08
  → green "✓ all pacing ahead". Neutral range → no badge (don't clutter).
- Badge appears after all TwoLine window rows when total==shown_count
  (all windows fit — no "+ N more" footer), on a blank separator line.
- Minimax label column widened 20→24 chars.
- `short_model_name` in minimax.rs: removed the hard 18-char truncation
  (was a legacy limit from the old 12-char label column). Now
  `truncate_suffix(model, 24)` in the dashboard handles display clipping.
  `Hailuo-2.3-Fast-6s-768p` (22 chars) now shows in full.

**Observations**
1. Badge fires correctly: Kimi shows "✓ all pacing ahead" (both windows
   ahead), Z.ai neutral (no badge), Codex neutral (45% used vs 48% elapsed
   — just inside ±8% threshold), Claude unavailable (no badge).
2. In this capture Claude was rate-limited → shows Unavailable card.
   Visual-order sort moves it to first position (lowest weight = 5 for
   Unavailable vs 6+ for available cards). Grid adapts cleanly.
3. `Hailuo-2.3-Fast-6s-768p` now untruncated on minimax card. Full model
   names visible across the board.
4. Badge line sits on a blank separator so it visually reads as a footer
   rather than another window row. Clean.
5. Codex card has 1 trailing empty line inside the border (row height driven
   by max card). Acceptable — trivial whitespace.

**Ideas queued for iter 9**
- [ ] Grid hint: add h/j/k/l mention or accept as power-user undocumented.
- [ ] When badge fires for burning-fast, color the card header/border orange
      as an alert indicator.
- [ ] Credits window (claude extra_credits) has no reset_at — consider
      displaying "no expiry" or just dropping the reset line.
