# Changelog

## 0.8.0 - 2026-04-17

- Added normal and compact detail layouts, with automatic compact fallback on smaller terminals and a manual `Tab` override.
- Moved plan/subscription and freshness metadata into the detail header so the most important status stays above the fold.
- Added persisted provider favorites plus persisted per-provider quota favorites and hidden quotas in `~/.config/quotas/config.toml`.
- Added direct detail-view controls for favoriting and hiding quotas, with hidden quotas rendered as dim rows that can be restored in-place.
- Scoped periodic auto-refresh to the visible provider while detail view is open, while keeping manual refresh global.
- Refreshed the tracked dashboard and detail screenshots to cover the new TUI behavior.
