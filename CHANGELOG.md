# Changelog

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
