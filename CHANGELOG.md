# Changelog

## Unreleased

- Added **Grok / xAI** provider: reuses Grok Build's `~/.grok/auth.json` session; fetches both monthly $ allowance (`/v1/billing`) and weekly product usage (`/v1/billing?format=credits`); Management API prepaid as fallback. Live fixtures under `tests/fixtures/grok/`.
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
