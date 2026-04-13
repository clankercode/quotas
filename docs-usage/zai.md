# Z.ai / Zhipu / GLM -- account usage endpoints

Research date: 2026-04-12

## Surface 1: Zhipu PAYG (open.bigmodel.cn)

**No documented billing/balance query API exists.**

The Zhipu PAYG platform (open.bigmodel.cn) bills per-token in CNY. Users top
up credit and consume it. The web console at
`https://open.bigmodel.cn/finance-center/finance/pay` displays balance and
transaction history, but **no public REST API for querying CNY credit balance
has been documented** in official docs, the Python SDK (zhipuai), the Java SDK,
or any third-party integration examined.

The chat completion response body includes standard OpenAI-compatible
`usage` fields:

```json
{
  "usage": {
    "prompt_tokens": 42,
    "completion_tokens": 128,
    "total_tokens": 170,
    "prompt_tokens_details": { "cached_tokens": 0 }
  }
}
```

These are per-response token counts, not account-level balance.

**Confidence: HIGH** that no public billing balance API exists for PAYG.
Multiple SDK reviews, community integrations (LangChain, LiteLLM, Spring AI,
Vercel AI SDK), and third-party tooling (openusage, onWatch) all skip PAYG
balance and focus exclusively on the Coding Plan monitor endpoints. The
official docs (docs.bigmodel.cn) document only the `/api/paas/v4` inference
endpoints with no finance/account section.


## Surface 2: Z.ai International (api.z.ai)

The international surface mirrors open.bigmodel.cn for the PAYG inference API
at `https://api.z.ai/api/paas/v4`. It shares the same lack of a documented
billing balance endpoint.

However, Z.ai **does** expose the same monitor endpoints described in Surface 3
below. The only difference is the base URL:

| Platform   | Base URL                                |
|------------|-----------------------------------------|
| Z.ai       | `https://api.z.ai/api/monitor/usage/`   |
| Zhipu (CN) | `https://open.bigmodel.cn/api/monitor/usage/` |
| Zhipu (dev)| `https://dev.bigmodel.cn/api/monitor/usage/`  |

Both Z.ai and Zhipu CN surfaces accept the same auth and return the same
response shape. The monitor endpoints work for Coding Plan subscribers; it
is unknown whether they return anything useful for pure PAYG users.

**Confidence: HIGH** for URL equivalence between Z.ai and Zhipu CN.


## Surface 3: Zai Coding Plan subscription

The Coding Plan is a subscription (Lite / Pro / Max) with rolling token
quotas rather than per-token billing. **Three undocumented but well-tested
monitor endpoints** are available, plus a subscription list endpoint.

### Authentication (all endpoints)

```
Authorization: Bearer <api_key>
Accept: application/json
Content-Type: application/json
```

The API key is the same one used for chat completions. Resolved via
`ZHIPU_API_KEY` env var or `~/.api-zai` file (in this repo's plugin:
`load_api_key(cfg, default_env="ZHIPU_API_KEY", default_file="~/.api-zai")`).

**Important:** Some community tools (opencode-glm-quota) pass the token
*without* the "Bearer" prefix. The openusage reference and the VS Code
extension both document `Authorization: Bearer <key>`. Test both if one
fails; the server may accept either form.

### 3a. GET /api/biz/subscription/list

Returns active subscription details.

**Response fields (observed):**

```json
{
  "code": 200,
  "msg": "操作成功",
  "success": true,
  "data": [
    {
      "productName": "GLM Coding Max",
      "status": "active",
      "purchaseTime": "...",
      "valid": true,
      "autoRenew": true,
      "currentRenewTime": "2026-03-12",
      "nextRenewTime": "2026-04-12",
      "billingCycle": "monthly",
      "paymentChannel": "..."
    }
  ]
}
```

`nextRenewTime` is the monthly MCP quota reset date (ISO date string).

**Confidence: MEDIUM.** Documented by openusage; field names match reports
from multiple independent projects. Not in official docs.

### 3b. GET /api/monitor/usage/quota/limit

The primary quota endpoint. Returns current consumption across all quota
dimensions.

**Response structure:**

```json
{
  "code": 200,
  "msg": "操作成功",
  "success": true,
  "data": {
    "level": "pro",
    "limits": [
      {
        "type": "5h Token",
        "rawType": "TOKENS_LIMIT",
        "unit": 3,
        "number": 5,
        "usage": 1000000,
        "currentValue": 72000,
        "remaining": 928000,
        "percentage": 7,
        "nextResetTime": 1712956800000,
        "total": 1000000
      },
      {
        "type": "Weekly Token",
        "rawType": "TOKENS_LIMIT",
        "unit": 6,
        "number": 7,
        "usage": 5000000,
        "currentValue": 2650000,
        "remaining": 2350000,
        "percentage": 53,
        "nextResetTime": 1713388800000
      },
      {
        "type": "MCP usage(1 Month)",
        "rawType": "TIME_LIMIT",
        "unit": 5,
        "number": 1,
        "usage": 1000,
        "currentValue": 42,
        "remaining": 958,
        "percentage": 4,
        "usageDetails": [
          { "modelCode": "search-prime", "usage": 20 },
          { "modelCode": "web-reader", "usage": 15 },
          { "modelCode": "zread", "usage": 7 }
        ]
      }
    ]
  }
}
```

**Field semantics:**

| Field             | Meaning |
|-------------------|---------|
| `rawType`         | `TOKENS_LIMIT` = token quota; `TIME_LIMIT` = MCP call quota |
| `unit`            | Period unit: `3` = hours, `5` = months, `6` = weeks |
| `number`          | Period multiplier (e.g. unit=3, number=5 -> 5-hour window) |
| `usage`           | Total quota ceiling (tokens or call count) |
| `currentValue`    | Amount consumed in current period |
| `remaining`       | `usage - currentValue` |
| `percentage`      | Consumption ratio 0-100 |
| `nextResetTime`   | Epoch milliseconds; absent for monthly TIME_LIMIT (resets 1st of month UTC) |
| `usageDetails`    | Per-MCP-tool breakdown (only on TIME_LIMIT entries) |
| `level`           | Subscription tier: `"lite"`, `"pro"`, `"max"`, or `"unknown"` |

**Quota dimensions by plan:**

| Plan | 5-Hour Tokens | Weekly Tokens | MCP Calls/Month |
|------|---------------|---------------|-----------------|
| Lite | ~120 prompts  | ~400 prompts  | 100             |
| Pro  | ~400 prompts  | ~2,000 prompts| 1,000           |
| Max  | ~1,600 prompts| ~8,000 prompts| 4,000           |

Note: "prompts" is the user-facing unit. One prompt triggers ~15-20 model
calls internally; the API reports raw tokens, not prompt counts.

**Confidence: HIGH.** Confirmed by at least five independent implementations:
opencode-glm-quota, openusage, zai-usage-tracker (VS Code), onWatch, and
zai-quota. The cc-switch issue (#1588) includes a real response example.

### 3c. GET /api/monitor/usage/model-usage?startTime=...&endTime=...

Returns per-model token usage within a time window.

**Query parameters:** `startTime` and `endTime` as ISO 8601 strings
(e.g., `2026-04-11T14:00:00`, `2026-04-12T14:59:59`). Typically queried
for a 24-hour rolling window.

**Response fields (from opencode-glm-quota):**

```json
{
  "code": 200,
  "data": {
    "totalUsage": {
      "totalTokensUsage": 145200,
      "totalModelCallCount": 37
    }
  }
}
```

May also include per-model breakdowns (model code -> tokens, calls).

**Confidence: MEDIUM.** Structure inferred from opencode-glm-quota source
code parsing and zai-usage-tracker README. No raw response example found.

### 3d. GET /api/monitor/usage/tool-usage?startTime=...&endTime=...

Returns MCP tool call usage within a time window.

**Response fields:**

```json
{
  "code": 200,
  "data": {
    "totalUsage": {
      "totalNetworkSearchCount": 12,
      "totalWebReadMcpCount": 8,
      "totalZreadMcpCount": 3
    }
  }
}
```

**Confidence: MEDIUM.** Same sourcing as model-usage.


## Rate-limit headers (all surfaces)

### Chat completion response headers

Z.ai's official documentation **does not document** any `x-ratelimit-*` or
`retry-after` headers on chat completion responses. This contrasts with
OpenAI and Anthropic which document these extensively.

Community tooling (OpenClaw, liteLLM) does **not** parse rate-limit headers
from Z.ai -- they all rely on catching 429 errors reactively.

### 429 error codes (documented)

Z.ai returns HTTP 429 with a business code in the JSON body:

| Business Code | Meaning | Reset |
|---------------|---------|-------|
| 1302          | Concurrency too high | Reduce parallel requests |
| 1303          | Request frequency too high | Slow down |
| 1304          | Daily call limit reached | Contact support |
| 1305          | Rate limit triggered | Unspecified |
| 1308          | Usage quota exhausted (5h/weekly) | Resets at `nextResetTime` |
| 1310          | Weekly/monthly limit exhausted | Resets at `nextResetTime` |

Source: https://docs.z.ai/api-reference/api-code

The Coding Plan's primary constraint is **concurrency** (in-flight requests),
not RPM/TPM. Reports indicate Pro tier allows only 1-2 concurrent requests,
causing "Too much concurrency" errors under agentic workloads.

**Confidence: HIGH** for error codes (from official docs). **HIGH** for
absence of rate-limit response headers (confirmed by multiple integrations
not parsing them).


## Open questions / unknowns

1. **PAYG balance API**: No programmatic way found to query CNY credit
   balance. The web console at `bigmodel.cn/finance-center/` presumably
   calls a backend API, but no one has reverse-engineered it publicly.
   The console URL is `https://open.bigmodel.cn/finance-center/finance/pay`.
   A future approach: intercept XHR requests in browser DevTools to discover
   the endpoint. The auth would likely be a session cookie, not an API key.

2. **Bearer prefix**: The opencode-glm-quota plugin explicitly notes "NO
   Bearer prefix" in its auth header, while openusage documents
   `Authorization: Bearer <key>`. Both reportedly work. The server may
   accept either form. Our plugin should try `Bearer <key>` first (standard
   convention) and fall back to bare key if needed.

3. **Monitor endpoints for PAYG users**: It is unknown whether
   `/api/monitor/usage/quota/limit` returns anything for PAYG-only accounts
   (no Coding Plan subscription). The `level` field might be `"unknown"` or
   the endpoint might 403.

4. **Response field stability**: All monitor endpoints are undocumented
   internal APIs. Field names or structure could change without notice.
   Z.ai has started providing an official usage query plugin
   (`glm-plan-usage` via `zai-org/zai-coding-plugins`) which suggests they
   are stabilizing these endpoints, but no formal API contract exists.

5. **Exact token limits per tier**: The API returns raw token ceilings in
   the `usage` field, but exact values are not published. They are
   approximated in prompt-equivalent terms in official FAQ.

6. **Weekly quota reset**: The 7-day window is described as "rolling from
   subscription activation" but `nextResetTime` is present, suggesting a
   fixed window. Needs empirical verification.

7. **Rate-limit headers**: Z.ai may add `x-ratelimit-*` headers in the
   future (they follow OpenAI-compat patterns), but as of 2026-04-12 there
   is no evidence they do.


## Implementation notes for thin plugin

The existing `provider_zai_coding_plan` plugin at
`src/thin_plugins/provider_zai_coding_plan/plugin.py`:
- Uses `DEFAULT_BASE_URL = "https://api.z.ai/api/coding/paas/v4"`
- Resolves key via `load_api_key(cfg, default_env="ZHIPU_API_KEY", default_file="~/.api-zai")`

A usage plugin should:
1. Reuse the same API key (ZHIPU_API_KEY / ~/.api-zai).
2. Hit `https://api.z.ai/api/monitor/usage/quota/limit` (or CN equivalent
   based on config).
3. Parse the `limits` array, identifying 5h token, weekly token, and MCP
   quotas by `rawType` + `unit` + `number`.
4. Optionally hit `/api/biz/subscription/list` for plan name and renewal date.
5. Report: percentage used, tokens remaining, time until reset (from
   `nextResetTime` epoch ms).


## Sources

- [opencode-glm-quota](https://github.com/guyinwonder168/opencode-glm-quota) -- OpenCode plugin with full TypeScript implementation querying all three monitor endpoints. Most detailed community reference.
- [openusage docs/providers/zai.md](https://github.com/robinebers/openusage/blob/main/docs/providers/zai.md) -- Documents subscription/list endpoint, quota/limit response fields, and auth. Best structured reference.
- [zai-usage-tracker VS Code extension](https://github.com/melon-hub/zai-usage-tracker) -- Confirms endpoints and response parsing; uses quota/limit + model-usage.
- [cc-switch issue #1588](https://github.com/farion1231/cc-switch/issues/1588) -- Contains real response JSON from quota/limit endpoint.
- [onWatch](https://github.com/onllm-dev/onwatch) -- Multi-provider usage tracker confirming ZAI_BASE_URL and quota/limit polling.
- [Z.ai error codes](https://docs.z.ai/api-reference/api-code) -- Official docs for HTTP error codes including 429 variants.
- [Z.ai rate limits page](https://z.ai/manage-apikey/rate-limits) -- Confirms concurrency-based limiting.
- [Z.ai coding plan FAQ](https://docs.bigmodel.cn/cn/coding-plan/faq) -- Official plan tier limits (5h, weekly, MCP monthly).
- [Z.ai usage query plugin](https://docs.z.ai/devpack/extension/usage-query-plugin) -- Official Claude Code plugin for quota queries, confirming Z.ai endorses this use case.
- [opencode issue #8618](https://github.com/anomalyco/opencode/issues/8618) -- Concurrency limit of 1 on Coding Plan Pro.
- [Zhipu API introduction](https://docs.bigmodel.cn/cn/api/introduction) -- Official docs confirming only /api/paas/v4 endpoints documented; no finance API.
- [yezhouguo/zai-quota](https://github.com/yezhouguo/zai-quota) -- Python script for Claude Code querying 5h and MCP quotas.
