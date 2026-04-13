# quotas — Implementation Plan

## Goal

Build a Rust CLI tool `quotas` that auto-detects AI provider credentials on the system, queries usage/quota APIs concurrently, and displays results in an interactive Textual TUI or a filterable `--json` headless format.

## Decisions Locked

| Decision | Choice |
|----------|--------|
| Language | Rust |
| TUI Framework | `textual` (async, reactive) |
| HTTP | `reqwest` |
| CLI Args | `clap` |
| Distribution | `cargo install quotas` |
| Providers | All 5: MiniMax, Z.ai, Kimi, Copilot, Codex |
| Auth | Auto-detect (env → config file → OAuth cache) |
| Unavailable provider | Shows "Unavailable" card with web console URL |
| TUI Style | Interactive (arrow keys + Enter drill-down + R refresh) |
| Staleness | Per-card "Updated Xs ago" badge; yellow >2min, red >5min |
| Clipboard | Press `C` to copy selected provider's JSON |
| JSON filtering | `--provider=` flag for targeted queries |

## Directory Structure

```
quotas/
├── Cargo.toml
├── src/
│   ├── main.rs              # clap CLI, mode dispatcher
│   ├── lib.rs               # Error type, shared config
│   ├── providers/
│   │   ├── mod.rs           # Provider enum, Provider trait
│   │   ├── minimax.rs       # MiniMax (io + CN fallback)
│   │   ├── zai.rs           # Z.ai / Zhipu Coding Plan
│   │   ├── kimi.rs          # Kimi PAYG + Coding Plan
│   │   ├── copilot.rs       # GitHub Copilot
│   │   └── codex.rs         # Codex / ChatGPT subscription
│   ├── auth/
│   │   ├── mod.rs           # AuthResolver trait
│   │   ├── env.rs           # Env var scanner
│   │   ├── file.rs          # Config file scanner
│   │   └── oauth.rs         # OAuth token cache reader
│   ├── tui/
│   │   ├── app.rs           # Textual App root
│   │   ├── dashboard.rs     # Main screen: provider grid
│   │   ├── detail.rs        # Drill-down screen
│   │   ├── provider_card.rs # Reusable card widget
│   │   ├── usage_bar.rs     # Progress bar widget
│   │   └── freshness.rs     # Staleness badge widget
│   └── output/
│       └── json.rs          # Headless JSON serializer
```

## Auth Discovery Map

```
MiniMax  → MINIMAX_API_KEY env | ~/.minimax config file
Z.ai     → ZHIPU_API_KEY env | ~/.api-zai file
Kimi     → MOONSHOT_API_KEY env | KIMI_API_KEY env
Copilot  → GITHUB_TOKEN env | gh auth token | ~/.config/github-copilot/ tokens
Codex    → OPENAI_API_KEY env | ~/.codex/auth.json (OAuth bearer)
```

## Provider API Endpoints

| Provider | Endpoint | Notes |
|----------|----------|-------|
| MiniMax (intl) | `GET /v1/api/openplatform/coding_plan/remains` | Bearer key; `current_interval_usage_count` = remaining (mislabeled) |
| MiniMax (CN) | Same on `api.minimaxi.com` | Broken (1004 cookie error); mark unavailable |
| Z.ai | `GET /api/monitor/usage/quota/limit` | Bearer; parse `limits[]` by `rawType`+`unit` |
| Kimi PAYG | `GET /v1/users/me/balance` | Bearer; returns USD balance |
| Kimi Coding | `GET /coding/v1/usages` | Bearer; 5h + weekly windows |
| Copilot | `GET /copilot_internal/user` | OAuth `token gho_xxx`; parse `quota_snapshots.premium_interactions` |
| Codex | `GET /backend-api/wham/usage` | OAuth or Bearer; parse `rate_limit` + `credits` |

## TUI Behavior

- **Startup**: Detect all providers → fetch all concurrently → display grid
- **Auto-refresh**: Every 60s, re-fetch all providers
- **Navigation**: `↑↓←→` move focus between cards; `Enter` drill into detail; `R` manual refresh; `C` copy JSON; `Q` quit
- **Staleness**: Per-card timer. `>2min` yellow warning badge. `>5min` red "STALE" badge.
- **Unavailable cards**: Render with grey background, "Unavailable" message, and console URL button.

## JSON Mode

```bash
quotas --json                          # all providers
quotas --json --provider=minimax,kimi  # only specific ones
quotas --json --provider=zai --pretty  # pretty-printed
```

## Implementation Phases

**Phase 1 — Foundation**: `cargo new quotas` + Cargo.toml + error types + auth discovery
**Phase 2 — Providers**: All 5 providers (MiniMax, Z.ai, Kimi, Copilot, Codex)
**Phase 3 — TUI**: Textual dashboard with interactive cards + detail drill-down
**Phase 4 — JSON Output**: `--json` flag + `--provider` filter + `--pretty`
**Phase 5 — Polish**: Clipboard, staleness, error states, `cargo install` metadata
