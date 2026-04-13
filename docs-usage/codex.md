# Codex / ChatGPT backend — account usage endpoints

Research date: 2026-04-12. Confidence levels noted per section.

---

## Surface 1: Codex CLI (ChatGPT subscription)

### Endpoint

| Property | Value |
|----------|-------|
| **URL (ChatGPT backend)** | `https://chatgpt.com/backend-api/wham/usage` |
| **URL (Codex API)** | `https://<codex-backend>/api/codex/usage` |
| **Method** | `GET` |
| **Auth header** | `Authorization: Bearer <access_token>` |
| **Account header** | `ChatGPT-Account-Id: <account_id>` (when available) |
| **User-Agent** | Codex sets a custom UA via `get_codex_user_agent()` |

The Codex CLI determines path style automatically: if the base URL contains
`/backend-api` it uses `/wham/usage`; otherwise `/api/codex/usage`. Both
return the same payload shape. The CLI normalises `chatgpt.com` and
`chat.openai.com` to append `/backend-api`.

**Confidence: HIGH** — directly read from
[`codex-rs/backend-client/src/client.rs`](https://github.com/openai/codex/blob/main/codex-rs/backend-client/src/client.rs)
in the public openai/codex repo (OpenAPI-generated models).

### Authentication

The Codex CLI authenticates via a browser-based OAuth flow (`codex login`)
which stores an access token at `~/.codex/auth.json` (or OS keyring). The
token carries the ChatGPT plan tier. Alternative: `codex login --with-api-key`
or `codex login --device-auth` for headless environments.

The auth.json file contains the bearer token and optionally the account ID.
The `CodexAuth` type exposes `get_token()` and `get_account_id()`.

### Response shape — `RateLimitStatusPayload`

```jsonc
{
  "plan_type": "plus",          // enum: guest|free|go|plus|pro|free_workspace|team|
                                //       self_serve_business_usage_based|business|
                                //       enterprise_cbp_usage_based|education|quorum|k12|enterprise|edu
  "rate_limit": {               // nullable — RateLimitStatusDetails
    "allowed": true,            // bool — can the user make requests right now?
    "limit_reached": false,     // bool — has the user hit any limit?
    "primary_window": {         // nullable — RateLimitWindowSnapshot (short window, ~5h)
      "used_percent": 23,       // i32 — 0-100, percent of window consumed
      "limit_window_seconds": 18000,  // i32 — window duration (18000 = 5 hours)
      "reset_after_seconds": 12345,   // i32 — seconds until this window resets
      "reset_at": 1744502400          // i32 — Unix epoch seconds when window resets
    },
    "secondary_window": {       // nullable — RateLimitWindowSnapshot (long window, ~weekly)
      "used_percent": 45,
      "limit_window_seconds": 604800, // 7 days
      "reset_after_seconds": 302400,
      "reset_at": 1744934400
    }
  },
  "credits": {                  // nullable — CreditStatusDetails
    "has_credits": true,        // bool — is credit tracking active?
    "unlimited": false,         // bool — Pro plans may have unlimited credits
    "balance": "42.50",         // nullable string — remaining credit balance
    "approx_local_messages": null,   // nullable array — approximate local msgs remaining
    "approx_cloud_messages": null    // nullable array — approximate cloud tasks remaining
  },
  "additional_rate_limits": [   // nullable array — AdditionalRateLimitDetails
    {
      "limit_name": "code_reviews",
      "metered_feature": "codex-other",
      "rate_limit": { /* same RateLimitStatusDetails shape */ }
    }
  ]
}
```

**Confidence: HIGH** — field names and types taken verbatim from the
OpenAPI-generated Rust models:
- [`rate_limit_status_payload.rs`](https://github.com/openai/codex/blob/main/codex-rs/codex-backend-openapi-models/src/models/rate_limit_status_payload.rs)
- [`rate_limit_status_details.rs`](https://github.com/openai/codex/blob/main/codex-rs/codex-backend-openapi-models/src/models/rate_limit_status_details.rs)
- [`rate_limit_window_snapshot.rs`](https://github.com/openai/codex/blob/main/codex-rs/codex-backend-openapi-models/src/models/rate_limit_window_snapshot.rs)
- [`credit_status_details.rs`](https://github.com/openai/codex/blob/main/codex-rs/codex-backend-openapi-models/src/models/credit_status_details.rs)
- [`additional_rate_limit_details.rs`](https://github.com/openai/codex/blob/main/codex-rs/codex-backend-openapi-models/src/models/additional_rate_limit_details.rs)

### Internal protocol type — `RateLimitSnapshot`

The CLI transforms the raw payload into a cleaner protocol type that is passed
to the TUI and SDK clients:

```typescript
// from codex-rs/app-server-protocol/schema/typescript/v2/

type RateLimitSnapshot = {
  limitId: string | null;       // "codex" for the primary bucket
  limitName: string | null;
  primary: RateLimitWindow | null;
  secondary: RateLimitWindow | null;
  credits: CreditsSnapshot | null;
  planType: PlanType | null;
};

type RateLimitWindow = {
  usedPercent: number;          // 0-100
  windowMinutes: number | null; // e.g. 300 (5h) or 10080 (weekly)
  resetsAt: number | null;      // Unix epoch seconds
};

type CreditsSnapshot = {
  hasCredits: boolean;
  unlimited: boolean;
  balance: string | null;       // e.g. "42.50"
};
```

The client calls `get_rate_limits()` which prefers the snapshot with
`limit_id == "codex"`, falling back to the first element.

### Reset schedule

| Window | Typical duration | Resets |
|--------|-----------------|--------|
| **Primary** | 5 hours (18,000 seconds) | Rolling — `reset_at` field gives exact Unix timestamp |
| **Secondary** | ~7 days (604,800 seconds) | Rolling weekly — `reset_at` field gives exact timestamp |

The `reset_at` field is a Unix epoch timestamp (seconds). The `reset_after_seconds`
field gives the countdown. Both are provided in the response.

Community reports indicate the weekly window can shift if you continue using
the service near the boundary. The exact mechanism appears to be a rolling
window, not a fixed calendar reset.

**Confidence: MEDIUM-HIGH** — window durations come from the code's default
display labels ("5h" for primary, "weekly" for secondary) and community
reports. The server could return other window sizes; the code handles arbitrary
`limit_window_seconds`.

### How the CLI displays usage

The TUI `/status` view renders usage as a 20-segment progress bar per window:

```
5h limit    [████████████░░░░░░░░] 40% left  resets at 3:45pm
Weekly limit [██████████████░░░░░░] 30% left  resets at Mon 4/14
Credits     42 credits
```

The CLI polls `/wham/usage` every 60 seconds via a background poller
(`prefetch_rate_limits`). Data older than 15 minutes is flagged as "stale".
There is no dedicated `codex usage` or `codex quota` subcommand — usage is
only visible in the interactive TUI status card and status line.

(Known issue: the poller fires even when using an API key, not ChatGPT auth —
see [#10869](https://github.com/openai/codex/issues/10869).)

### Quota numbers by plan

Per the [Codex pricing page](https://developers.openai.com/codex/pricing)
(April 2026):

| Plan | Primary window | Local messages (GPT-5.4 / mini / 5.3-Codex) | Cloud tasks | Code reviews |
|------|---------------|----------------------------------------------|-------------|-------------|
| **Free** | 5h | Limited (temporary) | — | — |
| **Plus** ($20/mo) | 5h | 20-100 / 60-350 / 30-150 | Shared | 5/hr |
| **Pro 5x** ($100/mo) | 5h | 200-1000 / 600-3500 / — | Shared | — |
| **Pro 20x** | 5h | 400-2000 / 1200-7000 / — | Shared | — |
| **Business** | pay-as-you-go | Token-based credits | Token-based | — |

Credits can be purchased to continue beyond included limits.

---

## Surface 2: OpenAI API key (api.openai.com)

This surface is for users with an `sk-...` API key billed per-token.

### Usage endpoint

| Property | Value |
|----------|-------|
| **URL** | `GET https://api.openai.com/v1/organization/usage/completions` |
| **Auth** | `Authorization: Bearer <OPENAI_ADMIN_KEY>` |
| **Docs** | [platform.openai.com/docs/api-reference/usage/completions](https://platform.openai.com/docs/api-reference/usage/completions) |

**Note:** This requires an **Admin API key**, not a regular project key.

#### Query parameters

| Param | Required | Description |
|-------|----------|-------------|
| `start_time` | Yes | Unix epoch seconds — start of query range |
| `end_time` | No | Unix epoch seconds — end of range |
| `bucket_width` | No | `1m`, `1h`, or `1d` (default `1d`) |
| `limit` | No | Max buckets returned |
| `page` | No | Pagination cursor |
| `project_ids` | No | Filter by project |
| `user_ids` | No | Filter by user |
| `api_key_ids` | No | Filter by API key |
| `models` | No | Filter by model name |
| `group_by` | No | Array: `model`, `project_id`, `user_id`, `api_key_id`, `batch` |

#### Response fields

```jsonc
{
  "data": [
    {
      "input_tokens": 12345,
      "output_tokens": 6789,
      "input_cached_tokens": 1000,
      "input_audio_tokens": 0,
      "output_audio_tokens": 0,
      "num_model_requests": 42,
      "project_id": "proj_xxx",
      "user_id": "user_xxx",
      "api_key_id": "key_xxx",
      "model": "gpt-4o",
      "batch": false
    }
  ],
  "has_more": false,
  "next_page": null
}
```

### Costs endpoint

| Property | Value |
|----------|-------|
| **URL** | `GET https://api.openai.com/v1/organization/costs` |
| **Auth** | Same Admin key |
| **Docs** | [Costs API cookbook](https://developers.openai.com/cookbook/examples/completions_usage_api) |

Returns `amount.value` (float, USD), `amount.currency`, `line_item`, `project_id`.

**Confidence: HIGH** — officially documented endpoints.

---

## Rate-limit headers

### OpenAI API (sk-... key) — on every response

| Header | Meaning |
|--------|---------|
| `x-ratelimit-limit-requests` | Max requests allowed in window |
| `x-ratelimit-limit-tokens` | Max tokens allowed in window |
| `x-ratelimit-remaining-requests` | Requests remaining before reset |
| `x-ratelimit-remaining-tokens` | Tokens remaining before reset |
| `x-ratelimit-reset-requests` | When request limit resets |
| `x-ratelimit-reset-tokens` | When token limit resets |

These headers appear on every chat completion response and can be read
without a separate API call.

### ChatGPT backend (Codex subscription)

The ChatGPT backend-api does **not** return standard `x-ratelimit-*` headers.
Rate-limit state is exclusively available via the `/wham/usage` polling
endpoint described above. The Codex CLI polls this every 60 seconds.

**Confidence: MEDIUM** — no evidence of `x-ratelimit-*` headers on the
ChatGPT backend in any source code or community report. The CLI exclusively
uses the polling endpoint.

---

## Implementation sketch for a thin plugin

```python
"""Minimal example: query Codex subscription usage."""

import json
import httpx

async def get_codex_usage(
    bearer_token: str,
    account_id: str | None = None,
    base_url: str = "https://chatgpt.com/backend-api",
) -> dict:
    headers = {
        "Authorization": f"Bearer {bearer_token}",
        "User-Agent": "thin-codex-usage/0.1",
    }
    if account_id:
        headers["ChatGPT-Account-Id"] = account_id

    async with httpx.AsyncClient() as client:
        resp = await client.get(f"{base_url}/wham/usage", headers=headers)
        resp.raise_for_status()
        return resp.json()

# Response has: plan_type, rate_limit.{allowed, limit_reached,
#   primary_window.{used_percent, reset_at, limit_window_seconds},
#   secondary_window.{...}},
#   credits.{has_credits, unlimited, balance}
```

---

## Existing code in this repo

- `src/thin_plugins/provider_openai/` — uses `sk-...` API keys via
  `https://api.openai.com/v1`. No usage/billing query support yet.
- `src/thin_plugins/provider_github_copilot/` — OAuth-based auth model that
  could serve as a pattern for ChatGPT OAuth token handling.
- **No Codex-specific provider plugin exists yet.** This research is pre-work
  for one.

---

## Open questions / unknowns

1. **Exact weekly window behaviour** — Community reports suggest the weekly
   reset can shift. Is it a true rolling window or a sliding window that
   anchors on first use? The `reset_at` field should clarify at runtime.

2. **`approx_local_messages` / `approx_cloud_messages` shape** — These fields
   on `CreditStatusDetails` are typed as `Vec<serde_json::Value>` (i.e., the
   OpenAPI generator couldn't infer a concrete type). Their internal structure
   is unknown. They likely contain per-model message estimates.

3. **Token refresh** — The ChatGPT OAuth token presumably expires. The Codex
   CLI's `CodexAuth` abstraction handles refresh, but the exact refresh
   endpoint and flow are not exposed in the public repo.

4. **Additional rate limits** — The `additional_rate_limits` array can carry
   extra metered features (e.g., code reviews). The set of possible
   `metered_feature` values is not documented.

5. **Admin key requirement for Usage API** — The `/v1/organization/usage/*`
   endpoints require an Admin key, not a regular project key. This limits
   self-service usage queries for users with only project-scoped keys.

6. **No CLI subcommand for usage** — There is no `codex usage` or
   `codex quota` command. Usage is only visible in the TUI status card.

---

## Sources

- [openai/codex `backend-client/src/client.rs`](https://github.com/openai/codex/blob/main/codex-rs/backend-client/src/client.rs) — `get_rate_limits()`, `get_rate_limits_many()`, URL construction, auth headers, payload mapping
- [openai/codex `codex-backend-openapi-models/src/models/`](https://github.com/openai/codex/tree/main/codex-rs/codex-backend-openapi-models/src/models) — `RateLimitStatusPayload`, `RateLimitStatusDetails`, `RateLimitWindowSnapshot`, `CreditStatusDetails`, `AdditionalRateLimitDetails`, `PlanType`
- [openai/codex `protocol/src/protocol.rs`](https://github.com/openai/codex/blob/main/codex-rs/protocol/src/protocol.rs) — `RateLimitSnapshot`, `RateLimitWindow`, `CreditsSnapshot` protocol types
- [openai/codex `tui/src/status/rate_limits.rs`](https://github.com/openai/codex/blob/main/codex-rs/tui/src/status/rate_limits.rs) — TUI rendering, progress bar, staleness detection
- [Issue #10869: Constant requests to `/wham/usage` even in API mode](https://github.com/openai/codex/issues/10869) — confirmed the polling endpoint
- [Codex pricing page](https://developers.openai.com/codex/pricing) — plan tiers, limits, credits
- [Codex auth docs](https://developers.openai.com/codex/auth) — OAuth flow, token storage
- [Codex CLI reference](https://developers.openai.com/codex/cli/reference) — no usage subcommand
- [OpenAI Usage API docs](https://platform.openai.com/docs/api-reference/usage/completions) — `/v1/organization/usage/completions` endpoint
- [Usage API cookbook](https://developers.openai.com/cookbook/examples/completions_usage_api) — practical examples and Costs API
- [OpenAI rate limits guide](https://developers.openai.com/api/docs/guides/rate-limits) — `x-ratelimit-*` headers
- [Using Codex with your ChatGPT plan](https://help.openai.com/en/articles/11369540-using-codex-with-your-chatgpt-plan) — plan details, reset behaviour
- [Issue #7354: Weekly usage limit refresh dates variable](https://github.com/openai/codex/issues/7354) — weekly reset shifting reports
- [Discussion #2251: Codex Usage Limits](https://github.com/openai/codex/discussions/2251) — community limit discussion
