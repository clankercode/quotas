# MiniMax -- account usage / billing / quota API

Research date: 2026-04-12

## Executive summary

MiniMax exposes **one documented usage endpoint**: `/v1/api/openplatform/coding_plan/remains`,
which returns request-count quotas for Token Plan (formerly "Coding Plan") subscriptions.
There is **no known public API** for querying Pay-As-You-Go (PAYG) account balance.
PAYG balance is visible only via the web console at
`https://platform.minimax.io/user-center/payment/balance`.

The `/coding_plan/remains` endpoint is **currently broken for Bearer-key auth** on the
China surface (`api.minimaxi.com` / `www.minimaxi.com`) -- it returns
`{"base_resp":{"status_code":1004,"status_msg":"cookie is missing, log in again"}}`.
The international surface (`api.minimax.io`) reportedly works with Bearer keys for
international-registered accounts (per GitHub issue #88 comments, Feb 2026).

---

## Surface 1: api.minimaxi.com (China)

### Endpoint

```
GET https://api.minimaxi.com/v1/api/openplatform/coding_plan/remains
```

Alternative hosts documented in various sources:
- `https://www.minimaxi.com/v1/api/openplatform/coding_plan/remains` (official CN FAQ)
- `https://platform.minimaxi.com/v1/api/openplatform/coding_plan/remains` (returns 404)

### Auth

```
Authorization: Bearer <TOKEN_PLAN_API_KEY>
Content-Type: application/json
```

The key is a Token Plan key (prefix `sk-cp-...` or similar). Standard PAYG keys
(`sk-api-...`) reportedly fail identically.

### Current status (April 2026)

**Broken.** Both host variants and both key types return HTTP 200 with:

```json
{"base_resp":{"status_code":1004,"status_msg":"cookie is missing, log in again"}}
```

This was confirmed by multiple OpenClaw contributors probing with real keys
(see openclaw/openclaw#52335). The endpoint appears to have drifted to require
a browser session cookie despite docs still advertising Bearer-key access.

**Confidence: HIGH** (multiple independent confirmations, April 2026)

---

## Surface 2: api.minimax.io (International)

### Endpoint

```
GET https://api.minimax.io/v1/api/openplatform/coding_plan/remains
```

### Auth

```
Authorization: Bearer <TOKEN_PLAN_API_KEY>
Content-Type: application/json
```

### Current status (April 2026)

**Reportedly working** for international Token Plan keys. A user in MiniMax-M2
issue #88 confirmed: "So the correct address for international Coder Plan is
`https://api.minimax.io/v1/api/openplatform/coding_plan/remains`". The
`minimax-usage-checker` macOS app (MIT, by AungMyoKyaw) uses this endpoint
successfully with GET + Bearer auth.

**Confidence: MEDIUM** (single user confirmation + working open-source app;
no independent verification by me since I lack a MiniMax key)

### Response shape

Confirmed from the `minimax-usage-checker` Swift models and OpenClaw source
(`provider-usage.fetch.minimax.ts`):

```json
{
  "base_resp": {
    "status_code": 0,
    "status_msg": ""
  },
  "model_remains": [
    {
      "model_name": "MiniMax-M2.7",
      "start_time": 1712937600000,
      "end_time": 1712955600000,
      "remains_time": 14400000,
      "current_interval_total_count": 200,
      "current_interval_usage_count": 187
    }
  ]
}
```

#### Field reference

| Field | Type | Meaning |
|---|---|---|
| `base_resp.status_code` | int | 0 = success. 1004 = auth/cookie error. |
| `base_resp.status_msg` | string | Human-readable error message. |
| `model_remains` | array | One entry per model in the plan. |
| `model_remains[].model_name` | string | e.g. `"MiniMax-M2.7"` |
| `model_remains[].start_time` | int64 | Window start, Unix ms |
| `model_remains[].end_time` | int64 | Window end, Unix ms |
| `model_remains[].remains_time` | int64 | Time left in window, ms |
| `model_remains[].current_interval_total_count` | int | Total request allowance in 5h window |
| `model_remains[].current_interval_usage_count` | int | **MISLABELED: actually REMAINING count** (see MiniMax-M2#99) |

**Critical note:** Despite its name, `current_interval_usage_count` contains the
*remaining* request count, NOT the consumed count. Consumed = total - usage_count.
This is a known documentation bug (MiniMax-AI/MiniMax-M2#99).

There may also be `current_weekly_total_count` and `current_weekly_usage_count`
fields (also mislabeled -- "usage" = remaining). These appear for plans with
weekly windows.

**Confidence: HIGH** (confirmed by open-source Swift models with CodingKeys,
OpenClaw source, and the official bug report)

---

## Pay-As-You-Go balance

### No known API endpoint

There is **no documented API** for querying PAYG account balance. The balance
is visible only in the web console:

- International: `https://platform.minimax.io/user-center/payment/balance`
- China: `https://platform.minimaxi.com/user-center/payment/balance`

The API overview at `platform.minimax.io/docs/api-reference/api-overview` lists
text, speech, video, image, music, and file endpoints. No billing or account
endpoints exist.

When PAYG balance hits zero, the chat API returns error code `1008`
("insufficient balance") as an HTTP 500:

```json
{"type":"error","error":{"type":"api_error","message":"insufficient balance (1008)"}}
```

or in the older response format:

```json
{"base_resp":{"status_code":1008,"status_msg":"insufficient balance"}}
```

**Confidence: HIGH** (exhaustive search of official docs, llms.txt index,
GitHub issues, and third-party integrations found no PAYG balance endpoint)

### Pricing units

PAYG billing is in **USD** (international) with per-token pricing. Approximately
1000 tokens = 1600 Chinese characters. No "credits" abstraction -- raw token
billing. Top-up via dashboard.

---

## Auth details

### Key types

MiniMax issues two key types from `platform.minimax.io/user-center/basic-information/interface-key`:

1. **Pay-as-you-go key** -- supports all modality models (text, video, speech, image)
2. **Token Plan key** -- subscription-based, supports same modalities

Keys are JWT-format strings (start with `ey...`). Both use Bearer auth.

### GroupId

The legacy v1 API required `GroupId` as a query parameter:

```
GET https://api.minimax.io/v1/text/chatcompletion_v2?GroupId={GROUP_ID}
```

The OpenClaw minimax-usage skill still sends `GroupId` in the query string for
the `/coding_plan/remains` call. However, the newer OpenAI-compatible endpoint
(`/v1/chat/completions`) and the `minimax-usage-checker` app do NOT use GroupId.
GroupId may be embedded in the JWT for newer keys.

The OpenClaw OAuth flow requests scope `"group_id profile model.completion"`,
suggesting group_id is still semantically relevant but embedded in the token.

### OAuth flow (portal auth)

MiniMax supports PKCE OAuth via:
- CN: `https://api.minimaxi.com/oauth/code` + `/oauth/token`
- International: `https://api.minimax.io/oauth/code` + `/oauth/token`

Grant type: `urn:ietf:params:oauth:grant-type:user_code`

This produces an `access_token` + `refresh_token`. It is unclear whether an
OAuth access token succeeds where a Bearer API key fails for the
`/coding_plan/remains` endpoint (OpenClaw has not yet confirmed this -- see
openclaw/openclaw#52335).

### Base URLs in existing plugin

The `provider_minimax` plugin in this repo uses:
```python
DEFAULT_BASE_URL = "https://api.minimax.io/v1"
```
Auth loaded from config or `~/.minimax` file.

---

## Rate-limit headers

### Documentation status

The official rate-limits page (`platform.minimax.io/docs/guides/rate-limits`)
documents RPM (requests per minute) and TPM (tokens per minute) limits per
model and plan tier, but does **not** specify response header names.

### What is known

- Multiple sources (Apidog, OpenClaw docs) reference monitoring
  `x-ratelimit-remaining` in MiniMax responses, suggesting MiniMax follows
  the standard `x-ratelimit-*` convention.
- The MiniMax MCP client sends a custom request header `MM-API-Source` (e.g.
  `"Minimax-MCP"` or `"OpenClaw"`). This is a request attribution header, not
  a response rate-limit header.
- Response header `Trace-Id` is confirmed present (used for error debugging in
  the official MiniMax MCP server code).

### Likely headers (unconfirmed exact names)

Based on the OpenAI-compatible surface and third-party references:

| Header | Likely meaning |
|---|---|
| `x-ratelimit-limit-requests` | RPM ceiling |
| `x-ratelimit-limit-tokens` | TPM ceiling |
| `x-ratelimit-remaining-requests` | Requests left in window |
| `x-ratelimit-remaining-tokens` | Tokens left in window |
| `x-ratelimit-reset-requests` | When request limit resets |
| `x-ratelimit-reset-tokens` | When token limit resets |
| `Trace-Id` | Request trace ID for debugging |

**Confidence: LOW** for exact header names. No first-party documentation or
confirmed response dumps available.

### Error codes for rate/quota exhaustion

| Code | Meaning |
|---|---|
| 1002 | Rate limit (RPM/TPM exceeded) |
| 1008 | Insufficient balance (PAYG) |
| 1039 | Token limit exceeded |
| 1041 | Concurrent connection limit |
| 2045 | Rate growth limit (too-rapid ramp) |
| 2056 | Usage limit exceeded (5h Token Plan window) |

**Confidence: HIGH** (from official error code docs)

---

## Reset schedule

### Token Plan (subscription)

- **Text models (M2.7 etc.)**: 5-hour rolling window. Usage older than 5 hours
  is automatically released. The window is tracked in `start_time`/`end_time`
  fields of the `/coding_plan/remains` response.
- **Non-text models (speech, video, image, music)**: Daily quotas that reset
  each day.
- Some plans may also have weekly windows (`current_weekly_total_count` /
  `current_weekly_usage_count`).

### PAYG

No reset. Pure consumption against a prepaid balance. Balance must be manually
recharged via the web console.

**Confidence: HIGH** (official Token Plan docs confirm 5h rolling / daily)

---

## How the web console displays balance

- **PAYG balance**: Shown at `/user-center/payment/balance` as a currency amount.
- **Token Plan usage**: Shown at `/user-center/payment/token-plan` (international)
  or `/user-center/payment/coding-plan` (older naming). Displays per-model
  usage percentages and remaining request counts within the current window.

The web console likely calls the same `/coding_plan/remains` endpoint
internally (with session cookies), which explains why the endpoint returns
"cookie is missing" when called with just a Bearer key on the CN surface.

---

## Recommendations for thin plugin

1. **For Token Plan users on the international surface**: Call
   `GET https://api.minimax.io/v1/api/openplatform/coding_plan/remains`
   with `Authorization: Bearer <key>`. Parse `model_remains` array.
   Remember that `current_interval_usage_count` means REMAINING, not used.

2. **For Token Plan users on the CN surface**: The endpoint is currently broken
   for Bearer auth. Consider:
   - Trying OAuth access tokens (untested)
   - Falling back to reporting "unavailable" with a link to the web console

3. **For PAYG users**: No API exists. The only option is:
   - Detect PAYG status by the absence of a `/coding_plan/remains` success
   - Report "PAYG -- check balance at [console URL]"
   - Watch for error code 1008 on chat requests as a signal of zero balance

4. **Host detection**: The plugin currently uses `api.minimax.io`. To support
   CN accounts, check if the key works against `api.minimaxi.com` as a
   fallback, or let the user configure the base URL.

5. **GroupId**: Try without it first (works for newer keys). If auth fails,
   try extracting group_id from the JWT payload and appending
   `?GroupId=<id>` to the request.

---

## Open questions / unknowns

- Does an OAuth access_token (from the PKCE user_code flow) work against
  `/coding_plan/remains` on the CN surface where Bearer API keys fail?
- Is there a separate PAYG balance endpoint behind the web console's session?
  (Would require browser network-tab reverse engineering to find out.)
- Exact `x-ratelimit-*` header names in MiniMax responses -- need a real API
  call to confirm.
- Whether `current_weekly_total_count` / `current_weekly_usage_count` appear
  for all Token Plan tiers or only certain ones.
- The old `GroupId` query parameter -- is it still required for some key types
  or fully deprecated?

---

## Sources

1. [MiniMax-AI/MiniMax-M2#88](https://github.com/MiniMax-AI/MiniMax-M2/issues/88) -- "API endpoint /coding_plan/remains requires cookie session instead of API Key"; confirmed `.io` works for international, `.com` fails with 1004
2. [MiniMax-AI/MiniMax-M2#99](https://github.com/MiniMax-AI/MiniMax-M2/issues/99) -- "`current_interval_usage_count` field is mislabeled -- returns remaining quota, not consumed usage"
3. [openclaw/openclaw#52335](https://github.com/openclaw/openclaw/issues/52335) -- "minimax-portal OAuth usage tracker reports 0% left"; maintainer live-probed all host/key combos, all return 1004 on CN
4. [openclaw/openclaw `provider-usage.fetch.minimax.ts`](https://github.com/openclaw/openclaw/blob/main/src/infra/provider-usage.fetch.minimax.ts) -- full implementation of usage parsing with field key lists and inversion logic
5. [AungMyoKyaw/minimax-usage-checker](https://github.com/AungMyoKyaw/minimax-usage-checker) -- macOS app; `CodingPlanModels.swift` defines exact response struct with CodingKeys
6. [openclaw/skills minimax-usage SKILL.md](https://github.com/openclaw/skills/blob/main/skills/thesethrose/minimax-usage/SKILL.md) -- usage skill using `platform.minimax.io/v1/api/openplatform/coding_plan/remains?GroupId={GROUP_ID}` with referer spoofing
7. [MiniMax Token Plan FAQ (CN)](https://platform.minimaxi.com/docs/token-plan/faq) -- official curl example: `GET https://www.minimaxi.com/v1/api/openplatform/coding_plan/remains`
8. [MiniMax Token Plan Intro](https://platform.minimax.io/docs/token-plan/intro) -- "5-hour rolling window" for text, "daily quotas" for non-text
9. [MiniMax Error Codes](https://platform.minimax.io/docs/api-reference/errorcode) -- error 1002, 1008, 1039, 1041, 2045, 2056
10. [MiniMax Pricing/PAYG](https://platform.minimax.io/docs/guides/pricing-paygo) -- USD per-token billing, no monthly reset
11. [MiniMax API Overview](https://platform.minimax.io/docs/api-reference/api-overview) -- no billing endpoints listed
12. [MiniMax-AI/MiniMax-Coding-Plan-MCP `client.py`](https://github.com/MiniMax-AI/MiniMax-Coding-Plan-MCP/blob/main/minimax_mcp/client.py) -- confirms `Trace-Id` response header, `MM-API-Source` request header, and base_resp error handling
13. [OpenClaw minimax `oauth.ts`](https://github.com/openclaw/openclaw/blob/main/extensions/minimax/oauth.ts) -- OAuth PKCE flow for CN (`api.minimaxi.com`) and global (`api.minimax.io`) with same client_id
14. [OpenClaw MiniMax provider docs](https://docs.openclaw.ai/providers/minimax) -- documents `coding_plan/remains` response fields and OpenClaw's inversion of usage_percent
