# quotas

A Rust CLI that auto-detects your AI provider credentials and shows usage
quotas for each provider in a single place — either as an interactive TUI or
as filterable JSON.

Supports five providers today:

| Provider | Auth discovery | Endpoint |
|----------|----------------|----------|
| **Claude** (Max/Pro subscription or API key) | `$CLAUDE_CONFIG_DIR/.credentials.json` → `~/.claude/.credentials.json` → `ANTHROPIC_API_KEY` | `GET /api/oauth/usage` on `api.anthropic.com` |
| **Codex** / ChatGPT subscription | `~/.codex/auth.json` → `OPENAI_API_KEY` | `GET /backend-api/wham/usage` on `chatgpt.com` |
| **MiniMax** Token Plan | `MINIMAX_API_KEY` → `~/.minimax` | `GET /v1/api/openplatform/coding_plan/remains` on `api.minimax.io` |
| **Kimi** (Coding Plan + PAYG) | `MOONSHOT_API_KEY` / `KIMI_API_KEY` | `GET /coding/v1/usages` and `/v1/users/me/balance` |
| **Z.ai / GLM** Coding Plan | `ZHIPU_API_KEY` / `ZAI_API_KEY` → `~/.api-zai` | `GET /api/monitor/usage/quota/limit` on `api.z.ai` |

## Install

```bash
cargo install quotas
```

## Usage

```bash
# Interactive TUI (default)
quotas

# Headless JSON for all providers
quotas --json

# Pretty-printed JSON
quotas --json --pretty

# Filter by provider
quotas --json --provider=claude,codex
```

TUI keybinds: `←↑↓→` navigate, `Enter` drill into a card, `R` refresh,
`C` copy selected provider's JSON to clipboard, `Q` quit.

## Output

Each provider result reports one of:

- **available** — authenticated and the usage API responded. Includes a
  plan name and one or more `QuotaWindow` entries (`5h`, `weekly`, etc.)
  with used/limit/remaining and reset timestamps.
- **auth_required** — no credentials were discoverable for that provider.
- **unavailable** — credentials worked but the server reported the plan
  is not active or the endpoint returned an error; includes a console URL.
- **network_error** — transport error reaching the endpoint.

## License

Dual-licensed under MIT OR Apache-2.0.
