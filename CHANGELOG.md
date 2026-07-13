# Changelog

## Unreleased

- **Kimi `totalQuota` semantics corrected from a live post-reset capture.** New fixture `tests/fixtures/kimi/coding_usages_post_reset.json` (kept alongside the frozen `coding_usages_default.json`) shows the healthy monthly-reset shape: `{limit:100, remaining:99}` with **`used` omitted**. `remaining` stays 99 in both frozen and healthy states, so deriving exhaustion from `limit - remaining` false-reds a healthy account. Freeze is now signalled only by `used` present and `>0`; remaining-only payloads render available (green). Calendar-month reset synthesis is kept as a best-effort hint but documented as approximate — this account's freeze→healthy transition landed mid-month (~2026-07-14), consistent with anniversary billing rather than the 1st. Tests: `parses_live_coding_usages_post_reset_fixture`; `parses_total_quota_from_remaining_only_when_used_missing` now expects available.

## 0.9.0 - 2026-07-12

- Added **Cursor** support sourced from your local Cursor credentials — no separate CLI login needed. The resolver reads the WorkOS access token from either the cursor-agent CLI (`~/.config/cursor/auth.json`) or the Cursor IDE's SQLite globalStorage (`~/.config/Cursor/User/globalStorage/state.vscdb`), and derives the user id from the token's JWT `sub`. Surfaces plan spend, bonus, and API/Auto usage windows, each stamped with the billing-cycle reset so the card shows a "resets in Xd Yh" hint and pace overlay. Live fixtures under `tests/fixtures/cursor/`.
- Refreshed the **MiniMax** parser for the current API payload (new `*_remaining_percent` / `*_status` fields; windows derived from the remaining-percent when `total_count` is `0`, and a depleted interval still renders as 100% used) and simplified the TUI layout by dropping the MiniMax-specific column/vertical spanning special-cases. Old payloads still parse. Live fixture `tests/fixtures/minimax/coding_plan_remains_live.json`.
- **Kimi `totalQuota`: synthesize calendar-month reset**. The upstream `/usages` endpoint reports the monthly membership cap without a server-side reset timestamp. The parser now synthesizes one at the next calendar-month boundary (1st of next month, 00:00 UTC) so the bar carries a "resets in Xd Yh" hint. `period_seconds` stays `None` — the binary bar shouldn't draw an overspend/slack overlay against elapsed calendar time. New helper `next_month_reset` is a pure function with its own tests (`next_month_reset_returns_first_of_next_month_mid_month`, `next_month_reset_rolls_year_on_december`, `next_month_reset_on_first_day_returns_next_month`). The 4 existing totalQuota tests now pin the synthesized `reset_at` to `2026-08-01T00:00:00Z` against a fixed test-clock of `2026-07-12T10:00:00Z`.
- **Kimi `totalQuota` collapsed to a binary signal**: the monthly membership cap window now renders as `limit=1, used=0|1` (raw `limit` ignored). The upstream `/usages` endpoint reports `used` on a meaningless 0/100+ scale, but only "any usage at all" matters — whether `used=1` or `used=50`, the result is the same: monthly cap touched, Kimi Code frozen. The bar now goes fully red the instant the cap is reached instead of showing a near-empty 1% / 50% bar. Empirically confirmed against a live capture from a genuinely frozen account on 2026-07-12: `totalQuota` reported `{limit:100, used:1, remaining:99}` while the 7d/5h windows still showed 100/100 — so `remaining` is deliberately ignored. (Standing caveat in the code/docs: re-verify a *healthy* mid-cycle account reports `used=0` after the next monthly reset.) The window is emitted from the presence of `used`/`remaining` rather than a non-zero `limit`, and its bar overlay shows just the percentage. Tests: `parses_total_quota_as_available_when_used_zero`, `parses_total_quota_ignores_raw_limit_value`, `parses_total_quota_from_remaining_only_when_used_missing`.
- Added **Grok / xAI** provider: reuses Grok Build's `~/.grok/auth.json` session; fetches both the monthly $ allowance (`/v1/billing`, rendered as a dollar-denominated pacing bar) and weekly product usage (`/v1/billing?format=credits`); Management API prepaid as fallback. Live fixtures under `tests/fixtures/grok/`.
- **Kimi fixtures refreshed**: captured a live `/coding/v1/usages` response into `tests/fixtures/kimi/coding_usages_default.json` and added `parses_live_coding_usages_fixture` test. Confirms the existing parser already handles string-typed numbers, `resetTime` (vs `reset_at`), and the `TIME_UNIT_`-prefixed time unit.
- **Kimi: surface the monthly membership cap** as a third quota window (`window_type: "total_quota"`, display label "total", sorted into the monthly bucket). The `/usages` endpoint's `totalQuota` field is the upstream cap that, when reached, freezes all Kimi Code requests per the [Kimi docs](https://www.kimi.com/code/docs/en/kimi-code/membership.html) — even though the 7d and 5h windows still report 100/100 remaining. Empirically confirmed on 2026-07-11: chat completions returned `403 access_terminated_error` with the new code while `/usages` showed full 7d/5h windows and `totalQuota.used=1/100`. Added `parses_kimi_coding_usages_without_total_quota` for the missing-field case.
- **Docs refreshed**: `docs-usage/kimi.md` updated with the empirically confirmed live response shape, including all new fields and a side-by-side diff vs the previously inferred shape.

## 0.8.3 - 2026-07-07

- Added a subtle colored TUI footer notice when the background startup update check finds a newer release.

## 0.8.2 - 2026-07-07

- Fixed statusline summaries so Claude shows the most constrained quota window, including active weekly Fable limits, instead of always reporting the first finite window.
- Refreshed Cargo dependencies to their latest compatible versions.

## 0.8.1 - 2026-07-06

- Added generic Claude subscription support for model-specific weekly limits reported either as top-level `seven_day_<model>` fields or scoped `limits[]` entries, including Fable.
- Improved quota labels so generic weekly model limits render cleanly in compact and sectioned TUI layouts.
- Documented Claude model-specific weekly quota windows in the README.
- Added `--version`, `--check-update`, and cached background crates.io update detection for the TUI/statusline.

## 0.8.0 - 2026-04-17

- Added normal and compact detail layouts, with automatic compact fallback on smaller terminals and a manual `Tab` override.
- Moved plan/subscription and freshness metadata into the detail header so the most important status stays above the fold.
- Added persisted provider favorites plus persisted per-provider quota favorites and hidden quotas in `~/.config/quotas/config.toml`.
- Added direct detail-view controls for favoriting and hiding quotas, with hidden quotas rendered as dim rows that can be restored in-place.
- Scoped periodic auto-refresh to the visible provider while detail view is open, while keeping manual refresh global.
- Refreshed the tracked dashboard and detail screenshots to cover the new TUI behavior.
