# Detail Favorites Refresh Design

## Scope

This design covers four related changes in the `quotas` TUI:

1. Add `normal` and `compact` detail-page layouts, with automatic compact fallback on smaller terminals and a manual override toggle.
2. Move freshness and subscription metadata higher in the detail header so the most important status information stays above the fold.
3. Persist provider favorites, quota favorites, and hidden quotas in config, and expose direct TUI controls for changing them.
4. Restrict detail-page auto-refresh to the currently visible provider instead of refreshing background providers while the user is focused on one provider.

Release preparation is part of the same work: version bump, changelog, artifacts, git tag, GitHub push/release, and `cargo publish`.

## Existing Constraints

- Dashboard state lives in `src/tui/dashboard.rs`, event handling lives in `src/main.rs`, and detail rendering lives in `src/tui/detail.rs`.
- Config is a single TOML file parsed in `src/config.rs`; there is currently no config-writing path.
- The project already has text snapshot rendering and tmux-driven screenshot capture. Those should be used to iterate on the layout until the narrow detail view is clean.
- Visual order for providers is intentionally stabilized after load. Favorite providers must integrate without causing reshuffles during refresh.

## Interaction Design

### Detail view modes

- Add two detail layouts:
  - `normal`: current multi-line quota rows, refined header, more breathing room.
  - `compact`: denser quota rows intended for smaller heights/widths.
- Add an auto-selection rule:
  - default to `compact` when the terminal is too short or too narrow for the normal layout to keep header metadata and at least one actionable quota row visible;
  - otherwise default to `normal`.
- Add a manual toggle key in detail view to override the auto choice for the current session.
- The header hint row should advertise the toggle.

### Detail header

- The top visible section should show:
  - provider name on the left, with a favorite marker if favorited;
  - subscription / plan name on the right when available;
  - a fixed-width freshness block on the far right containing:
    - relative freshness label;
    - compact progress bar for the next auto-refresh interval when auto-refresh is enabled.
- Auth source should stay directly below the title row.
- The old plan-name line should disappear from the scrolled body in normal cases because it moves into the header area.

### Provider and quota controls

- Provider favorites:
  - toggle from dashboard and detail view;
  - sort favorite providers first in dashboard order;
  - display a visual marker in both dashboard cards and detail header.
- Quota favorites:
  - toggle from detail view for the focused quota row;
  - sort favorites above non-favorites within that provider;
  - in compact mode, favorites stay pinned before any trimmed non-favorite rows;
  - display a visual marker on each favorite quota row.
- Hidden quotas:
  - toggle from detail view for the focused quota row;
  - hidden quotas are omitted from the main rendered quota list;
  - the detail view includes a dim one-line control row for each hidden quota so it can be unhidden without editing config.
- Focus model inside detail:
  - detail view gains a quota-row cursor separate from the provider selection;
  - `Up`/`Down` move the focused row when detail is open;
  - scrolling remains available through `PageUp`/`PageDown` and mouse wheel;
  - favorite/hide actions apply to the focused quota or hidden-quota control row.

### Recommended key bindings

- `Enter` / `Esc`: open-close detail, unchanged.
- `Tab`: toggle detail mode (`auto/normal/compact` resolved as a cycling session override).
- `f`: favorite/unfavorite current provider from dashboard or detail; in detail, if a quota row is focused, toggle quota favorite instead.
- `x`: hide/unhide focused quota row.
- `Shift+X` is not required; unhide is handled by focusing the dim hidden row and pressing `x`.
- `c`, `r`, `a`, `q`, left/right provider navigation remain.

To keep ambiguity low:
- on the dashboard, `f` always targets the selected provider;
- in detail, `f` targets the focused quota row if one exists, otherwise the provider;
- `x` is only active in detail.

## Data and Config Design

Add a persisted preferences section to config:

```toml
[favorites]
providers = ["codex", "claude"]

[quota_preferences.codex]
favorites = ["5h", "7d", "spark/7d"]
hidden = ["o3/weekly"]
```

Notes:

- Provider identifiers use provider slugs.
- Quota identifiers use a stable key derived from `QuotaWindow.window_type`.
- Preferences are per provider, persisted across runs.
- Quota matching is exact-string; no pattern language.

Implementation details:

- Introduce typed config structs for favorites and per-provider quota preferences.
- Add a config write path that preserves unrelated sections by deserializing-modifying-serializing the known config model.
- On each interactive toggle, write the config file immediately so a crash does not lose preferences.
- If the config directory does not exist, create it.

## Sorting and Visibility Rules

### Provider sorting

Compute visual order using:

1. favorited providers first;
2. existing status/meaningful-content ordering second;
3. original stable ordering as the final tie-breaker.

This preserves the dashboard’s current anti-jitter behavior while allowing favorites to rise to the top.

### Quota ordering

Visible quota rows in detail should be produced in this order:

1. favorite visible quotas;
2. non-favorite visible quotas;
3. hidden-quota control rows (dimmed).

Within each bucket, preserve the existing semantic sort order from `bar::window_sort_key`.

## Scoped Auto-Refresh Behavior

- Dashboard view keeps current per-provider auto-refresh behavior.
- Detail view changes behavior:
  - only the currently visible provider auto-refreshes;
  - other providers keep their existing cached state and timers but do not spawn background refresh work while detail is open;
  - when the user switches left/right between providers in detail, the newly visible provider becomes eligible for auto-refresh.
- Manual refresh (`r` / refresh button) still refreshes all fetchable providers.

This makes the detail page stable and avoids needless background churn while a single provider is under inspection.

## Rendering Strategy

- Extend `DetailView` to render from a richer view model instead of directly walking raw windows.
- Introduce a small detail-specific model:
  - header metadata;
  - ordered visible quota rows;
  - ordered hidden quota rows;
  - current focus row;
  - resolved display mode.
- Keep the dashboard renderer responsible for framing and keyboard hints; keep detail row rendering in `src/tui/detail.rs`.
- Use the existing snapshot and tmux screenshot commands to iterate at minimum on:
  - `200x60`
  - `160x50`
  - `120x40`
  - `80x30`
  - `80x20`

## Testing

Add and update tests for:

- config parse/serialize of favorites and hidden quotas;
- provider sorting with favorites;
- detail row ordering and hidden-row rendering;
- normal vs compact detail rendering thresholds;
- header placement of plan/freshness metadata;
- detail focus behavior and toggle semantics;
- auto-refresh gating when detail is open.

Also keep snapshot artifacts for the detail layout changes so regressions are visible in git.

## Release Flow

The repo does not currently contain a full release pipeline, so release work should be explicit:

1. update version numbers in `Cargo.toml` and lockfile;
2. add or update `CHANGELOG.md` with a release entry summarizing the new TUI behavior;
3. build release artifacts locally;
4. create a git tag matching the new version;
5. push commit + tag to GitHub;
6. create or verify a GitHub release containing changelog text and compiled artifacts;
7. run `cargo publish`.

If GitHub automation is absent or incomplete, perform the release manually.
