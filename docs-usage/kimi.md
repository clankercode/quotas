# Kimi / Moonshot -- account usage endpoints

Research date: 2026-04-12


## Surface 1: Kimi PAYG (api.moonshot.ai)

### Balance endpoint

The Moonshot Open Platform provides an **official, documented** balance query
endpoint.

| Detail            | Value |
|-------------------|-------|
| **URL**           | `GET https://api.moonshot.ai/v1/users/me/balance` |
| **CN mirror**     | `GET https://api.moonshot.cn/v1/users/me/balance` |
| **Auth**          | `Authorization: Bearer $MOONSHOT_API_KEY` |
| **Request body**  | None |
| **Docs**          | <https://platform.kimi.ai/docs/api/balance> (redirected from platform.moonshot.ai) |

#### Response shape (HTTP 200)

```json
{
  "code": 0,
  "scode": "0x0",
  "status": true,
  "data": {
    "available_balance": 49.58894,
    "voucher_balance": 46.58893,
    "cash_balance": 3.00001
  }
}
```

| Field                     | Type  | Unit | Notes |
|---------------------------|-------|------|-------|
| `data.available_balance`  | float | USD (`.ai`) / CNY (`.cn`) | Total usable funds. When <= 0 the inference API rejects requests. |
| `data.voucher_balance`    | float | same | Voucher/credit balance. Cannot be negative. |
| `data.cash_balance`       | float | same | Cash balance. **Can be negative** (debt). |

The English docs at `platform.kimi.ai` say "unit: USD". The CN docs at
`platform.kimi.com` (formerly `platform.moonshot.cn`) likely return CNY. The
currency matches the account region, not the endpoint domain.

#### Error responses

- **401** -- invalid or missing API key.
- **500** -- server error.

Error body: `{"error": {"message": "...", "type": "...", "code": "..."}}`.

#### Reset / billing-cycle

PAYG has no cycle or reset. Balance depletes as tokens are consumed. Recharge
via the platform console at any time.

**Confidence: HIGH** -- officially documented with example response.

### Rate-limit tiers

The PAYG platform enforces 6 tiers (Tier 0--5) based on cumulative recharge
amount ($1 to $3,000+). Limits are per-user (not per-key), shared across all
models. Four dimensions:

| Dimension     | Tier 0 | Tier 5 |
|---------------|--------|--------|
| Concurrency   | 1      | 1,000  |
| RPM            | 3      | 10,000 |
| TPM            | 500K   | 5M     |
| TPD            | 1.5M   | Unlimited |

When a limit is hit the API returns HTTP 429 with
`"type": "rate_limit_reached_error"`. The error message body embeds the
specific metric exceeded, e.g.:

```
Your account {uid}<{ak-id}> request reached TPM rate limit,
current:{current_tpm}, limit:{max_tpm}
```

No dedicated "get my current tier / remaining RPM" endpoint exists. The only
programmatic signal is the 429 error itself, the balance endpoint above, and
(possibly) response headers -- see the headers section below.


## Surface 2: Kimi For Coding subscription (api.kimi.com)

### /usages endpoint (reverse-engineered from kimi-cli)

The Kimi Code CLI (`MoonshotAI/kimi-cli`) ships a `/usage` slash command
(alias `/status`) that calls a REST endpoint to display quota. The endpoint
is **not documented in the official Kimi Code docs** but is confirmed in the
open-source kimi-cli codebase.

Source files (GitHub, MoonshotAI/kimi-cli):
- `src/kimi_cli/ui/shell/usage.py` -- the command implementation
- `src/kimi_cli/auth/platforms.py` -- platform definitions including base URL

| Detail            | Value |
|-------------------|-------|
| **URL**           | `GET https://api.kimi.com/coding/v1/usages` |
| **Auth**          | `Authorization: Bearer $KIMI_API_KEY` (sk-kimi-* token) |
| **Request body**  | None |
| **Docs**          | NONE (undocumented). Found via kimi-cli source and a Kimi Forum thread. |

The endpoint is constructed as `{platform.base_url}/usages` where
`platform.base_url` defaults to `https://api.kimi.com/coding/v1`
(overridable via env `KIMI_CODE_BASE_URL`).

A curl example was found in a Kimi Forum moderator post:

```bash
curl -H "Authorization: Bearer $KIMI_API_KEY" \
  https://api.kimi.com/coding/v1/usages
```

#### Response shape (inferred from kimi-cli parser)

The kimi-cli parser (`_parse_usage_payload` in `usage.py`) expects:

```json
{
  "usage": {
    "limit": <int>,
    "used": <int>,
    "remaining": <int>,
    "name": "Weekly limit",
    "reset_at": "2026-04-14T00:00:00.000Z"
  },
  "limits": [
    {
      "detail": {
        "limit": <int>,
        "used": <int>,
        "remaining": <int>,
        "name": "5h limit"
      },
      "window": {
        "duration": 300,
        "timeUnit": "MINUTE"
      }
    }
  ]
}
```

Field semantics derived from the parser:

| Field path                   | Type   | Notes |
|------------------------------|--------|-------|
| `usage.limit`                | int    | Total weekly token quota |
| `usage.used`                 | int    | Tokens consumed this cycle |
| `usage.remaining`            | int    | Tokens remaining (or `limit - used` fallback) |
| `usage.name` / `usage.title` | string | Human label, e.g. "Weekly limit" |
| `usage.reset_at`             | string | ISO 8601 timestamp when weekly quota resets |
| `limits[].detail.limit`     | int    | Sliding-window token cap |
| `limits[].detail.used`      | int    | Tokens consumed in current window |
| `limits[].window.duration`  | int    | Window size (e.g. 300) |
| `limits[].window.timeUnit`  | string | "MINUTE" / "HOUR" / "DAY" |

The parser also checks for `resetAt`, `reset_time`, `resetTime`, `reset_in`,
`resetIn`, `ttl`, and `window` fields as alternative reset hints -- suggesting
the server schema may vary.

**Units**: The kimi-cli UI renders "% left" progress bars with no unit label.
Given the recent switch to "token-based billing" (announced via @Kimi_Moonshot
on X, ~2026-03), the `limit`/`used` fields are likely **token counts**, not
request counts (despite older docs saying "2,048 requests/week").

#### Quota schedule

- **Weekly cycle**: 7-day rolling from subscription activation date (D1-D7,
  D8-D14, ...). Unused quota does NOT carry over.
- **5-hour sliding window**: Within the weekly cycle, a shorter rolling window
  caps burst usage. The kimi-cli parser handles `window.duration=300` +
  `window.timeUnit=MINUTE` (= 5 hours).
- **Concurrency cap**: 30 simultaneous requests.

#### Console

The web console at `https://www.kimi.com/code/console` (referenced in the 403
error message) presumably calls the same `/usages` endpoint or an equivalent.
No public documentation of its internal API calls was found.

**Confidence: MEDIUM** -- endpoint URL is confirmed in open-source code and a
forum post. Response shape is inferred from the parser, not from actual
captured responses. Token-vs-request unit is an educated guess based on the
billing switch announcement.


## Rate-limit headers (both surfaces)

### PAYG (api.moonshot.ai)

The official docs (introduction page, chat API page, rate limits page) **do not
document** `x-ratelimit-*` response headers. Multiple searches across the
official docs, CSDN, Juejin, and GitHub turned up no documented header names.

However, circumstantial evidence suggests they may exist:
- A Kimi Forum moderator mentioned "the `/usages` endpoint currently only
  reflects token limits" and that "concurrency limit tracking is still under
  development", implying header-based signals were incomplete.
- The OpenAI SDK's automatic retry logic works with Moonshot, which typically
  relies on `retry-after` or `x-ratelimit-*` headers.
- One third-party article (kimi-ai.chat) mentions "X-Credits-Remaining" and
  "X-RateLimit-Limit / X-RateLimit-Remaining" in passing, but this site is
  not official Moonshot documentation and may be speculative.

**Recommendation**: Make a real API call and inspect headers to confirm. A
thin plugin should log and parse any `x-ratelimit-*` headers opportunistically
but must not depend on them.

### Kimi Code (api.kimi.com)

No documentation or community reports of rate-limit headers on the Anthropic-
compat coding endpoint. The kimi-cli source does not parse response headers
for rate-limit data -- it relies entirely on the `/usages` endpoint.


## Implementation notes for a thin plugin

### Surface 1 (PAYG)

```python
# Reuses existing credentials from provider_kimi plugin
async def get_payg_balance(api_key: str, base_url: str = "https://api.moonshot.ai/v1") -> dict:
    url = f"{base_url}/users/me/balance"
    headers = {"Authorization": f"Bearer {api_key}"}
    # GET, parse response["data"]["available_balance"]
```

### Surface 2 (Coding Plan)

```python
# Reuses existing credentials from provider_kimi_coding_plan plugin
async def get_coding_usage(api_key: str, base_url: str = "https://api.kimi.com/coding/v1") -> dict:
    url = f"{base_url}/usages"
    headers = {"Authorization": f"Bearer {api_key}"}
    # GET, parse response["usage"] and response["limits"]
```

Both plugins already resolve API keys through their existing auth chains
(`load_api_key` for PAYG, 4-step precedence chain for Coding Plan). The usage
plugin can import those helpers directly.


## Open questions / unknowns

1. **Exact response shape of `/usages`**: The parser handles many field name
   variants (camelCase and snake_case). Without a captured real response, we
   cannot be 100% sure which variant the server currently returns.

2. **Rate-limit response headers**: Neither surface has documented headers.
   Need empirical testing (`curl -v`) to check.

3. **Currency on `.ai` vs `.cn` balance endpoint**: The English docs say USD;
   the CN docs presumably return CNY. Is the currency determined by account
   region or endpoint domain? Unknown.

4. **Token vs request units in `/usages`**: The March 2026 switch to
   "token-based billing" likely changed the unit from requests to tokens, but
   this is not confirmed in the response schema.

5. **Console API**: Does `kimi.com/code/console` hit `/usages` or a different
   endpoint? Could potentially discover via DevTools but no public record
   found.

6. **OAuth tokens**: The coding plan also supports OAuth device flow (not just
   static API keys). Does `/usages` work with OAuth access tokens? The kimi-cli
   code calls `resolve_api_key(provider.api_key, provider.oauth)` before
   hitting the endpoint, suggesting yes.


## Sources

1. [Moonshot balance endpoint (EN)](https://platform.kimi.ai/docs/api/balance) -- official docs, response schema with field names and units.
2. [Moonshot balance endpoint (CN)](https://platform.kimi.com/docs/api/balance) -- same content, CNY units, redirected from platform.moonshot.cn.
3. [Moonshot rate limits](https://platform.kimi.ai/docs/pricing/limits) -- tier table, 4 rate-limit dimensions, no header docs.
4. [Moonshot introduction / concepts](https://platform.kimi.ai/docs/introduction) -- rate limit enforcement model (user-level, shared across models).
5. [Moonshot FAQ](https://platform.kimi.ai/docs/guide/faq) -- 429 error message format with TPM/RPM details.
6. [kimi-cli usage.py](https://github.com/MoonshotAI/kimi-cli/blob/main/src/kimi_cli/ui/shell/usage.py) -- `/usages` endpoint construction and response parser.
7. [kimi-cli platforms.py](https://github.com/MoonshotAI/kimi-cli/blob/main/src/kimi_cli/auth/platforms.py) -- platform base URLs and IDs.
8. [kimi-cli auth/__init__.py](https://github.com/MoonshotAI/kimi-cli/blob/main/src/kimi_cli/auth/__init__.py) -- KIMI_CODE_PLATFORM_ID constant.
9. [Kimi Forum: 429 error thread](https://forum.moonshot.ai/t/error-code-429-were-receiving-too-many-requests-at-the-moment/191) -- curl example for `/usages`, moderator comment on concurrency tracking gaps.
10. [Kimi Code docs: benefits](https://www.kimi.com/code/docs/en/benefits.html) -- 7-day rolling cycle, 5-hour window, 30 concurrency, no carryover.
11. [Kimi Code docs: membership](https://www.kimi.com/code/docs/en/) -- plan overview, console URL.
12. [@Kimi_Moonshot on X](https://x.com/Kimi_Moonshot/status/2016918447951925300) -- announcement of permanent switch to token-based billing (~March 2026).
13. [kimik2ai.com pricing](https://kimik2ai.com/pricing/) -- Moderato plan $19/month, 2048 requests/week (pre-token-switch number).
14. [NxCode Kimi Code guide](https://www.nxcode.io/resources/news/kimi-code-2026-plans-pricing-developer-guide) -- 5-hour rolling window explanation, $19/month pricing.
15. [LiteLLM Moonshot provider](https://docs.litellm.ai/docs/providers/moonshot) -- community integration reference.
16. [openclaw issue #43447](https://github.com/openclaw/openclaw/issues/43447) -- 429 masking insufficient funds bug.
17. [DeepWiki kimi-cli usage tracking](https://deepwiki.com/MoonshotAI/kimi-cli/10.4-api-usage-tracking) -- high-level overview of the usage system.
18. [kimi-cli slash commands docs](https://moonshotai.github.io/kimi-cli/en/reference/slash-commands.html) -- /usage command description.
