# OpenCode Go -- subscription usage endpoint

Research date: 2026-10-10

## Surface: OpenCode Go subscription (Go / Go Plus)

OpenCode Go is OpenCode's subscription for open coding models — **Go**
($10/mo) and **Go Plus** ($40/mo). Each model gets a monthly dollar
allowance; the 5-hour rolling window is 20% of that allowance and the
weekly window is 50%. The usage API reports **only percentages, not
dollars**.

Provider slug: `opencode-go`. Selected via `--provider=opencode-go`
(aliases `opencodego`, `opencode_go`, `go`). To hide the card, disable
the slug in `~/.config/quotas/config.toml`:

```toml
[providers]
enabled = ["*"]
disabled = ["opencode-go"]
```

### Authentication

```
Authorization: Bearer <OpenCode Go API key>
User-Agent: quotas-cli/<version>
```

Discovery order:

1. `OPENCODE_GO_API_KEY` environment variable.
2. Bare-token file `~/.opencode-go`.
3. opencode auth file — `~/.local/share/opencode/auth.json` (also
   `$XDG_DATA_HOME/opencode/auth.json` and
   `~/.config/opencode/auth.json`), slot `opencode-go`. Written by
   `opencode auth login` / `/connect` → OpenCode Go.
4. pi agent `~/.pi/agent/auth.json` — fuzzy slot match on `opencode-go`,
   `opencodego`, or `opencode_go`.

**Important:** the `opencode` slot in those auth files is the OpenCode
**Zen** (pay-as-you-go) key — a different credential. quotas never uses
it for Go: the opencode resolver reads only the exact `opencode-go`
slot, and the pi fuzzy matcher deliberately excludes plain `opencode`.

### Endpoint

`GET https://opencode.ai/zen/go/v1/usage`

Undocumented; used by the opencode-go-limits plugin and documented in
cc-switch issue #6433 (see Sources).

**Response structure:**

```json
{
  "usage": {
    "rolling": { "status": "ok", "percent": 12, "resetsAt": "2026-10-11T04:08:19.000Z" },
    "weekly":  { "status": "ok", "percent": 34, "resetsAt": "2026-10-12T00:00:00.000Z" },
    "monthly": { "status": "ok", "percent": 56, "resetsAt": "2026-11-09T15:20:48.000Z" }
  }
}
```

**Field semantics:**

| Field      | Meaning |
|------------|---------|
| `status`   | `ok` or `rate-limited` |
| `percent`  | Used percent of that window (integer or float) |
| `resetsAt` | ISO-8601 UTC timestamp (with milliseconds) of the next reset |

**Windows shown:**

| TUI label | JSON `window_type` | API field | Reset |
|-----------|--------------------|-----------|-------|
| `5h`      | `5h`               | `rolling` | 5-hour rolling window (20% of the monthly allowance) |
| `7d`      | `weekly`           | `weekly`  | Mondays 00:00 UTC (50% of the monthly allowance) |
| `month`   | `monthly`          | `monthly` | Subscription anniversary |

Each window is normalized to `used / 100`: `percent` is clamped to 0–100
and rounded; a `rate-limited` window is reported as 100% used regardless
of `percent`. Every window carries `period_seconds` and `reset_at`, so
the card shows a reset countdown and the pace marker.

**User-Agent requirement:** the endpoint returns HTTP 403 (Cloudflare
error 1010, `browser_signature_banned`) to generic user agents, so
quotas sends `User-Agent: quotas-cli/<version>`.

### Error handling

- HTTP 401 `Unauthorized` / `Missing API key.` → the card shows
  `Unavailable: <message>` with the console link.
- No credentials found → the provider is listed under `No key:` in the
  footer.
- Console: https://opencode.ai/console/ (shown on the card when the key
  is rejected).

**Confidence: undocumented endpoint, verified live 2026-10-10.**


## Sources

- [OpenCode Go docs](https://opencode.ai/docs/go/) -- official docs for the Go / Go Plus plans.
- [opencode issue #16017](https://github.com/anomalyco/opencode/issues/16017) -- feature request for a public usage API (closed).
- [opencode issue #31084](https://github.com/anomalyco/opencode/issues/31084) -- feature request for a public usage API (closed).
- [cc-switch issue #6433](https://github.com/farion1231/cc-switch/issues/6433) -- documents the `/zen/go/v1/usage` response.
