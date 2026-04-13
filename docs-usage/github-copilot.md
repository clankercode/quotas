# GitHub Copilot — account usage endpoint

Last updated: 2026-04-12

## Endpoint

There are **two** distinct surfaces for querying Copilot usage.

### 1. Internal endpoint (live quota snapshot) — RECOMMENDED for plugin use

```
GET https://api.github.com/copilot_internal/user
```

This is the endpoint VS Code, Zed, and third-party tools (ericc-ch/copilot-api, LiteLLM)
call to display the "X of Y premium requests used" badge. It returns real-time quota
snapshots with remaining counts and percentages.

**Status:** Undocumented internal API. Not listed in docs.github.com REST reference.
Community-verified (Zed discussion #44499, ericc-ch/copilot-api, LiteLLM issue #18242).
Has been stable since at least early 2025.

### 2. Official billing endpoint (historical usage report)

```
GET https://api.github.com/users/{username}/settings/billing/premium_request/usage
```

Documented at `docs.github.com/en/rest/billing/usage`. Returns itemised billing line items
(product, SKU, model, unitType, pricePerUnit, quantities, amounts). Useful for
cost-accounting and historical queries, but does **not** return remaining-quota or
entitlement — only spend.

Required API version header: `X-GitHub-Api-Version: 2026-03-10`

Optional query params: `year`, `month`, `day`, `model`, `product`.

Only the past 24 months of data are accessible.

## Auth

### For the internal endpoint (`/copilot_internal/user`)

Uses the **same OAuth token** (`gho_*`) that the provider-github-copilot plugin already
resolves. The token must have the `read:user` scope (the standard Copilot OAuth app
scope).

```
Authorization: token gho_xxxxxxxxxxxxxxxxxxxxx
```

Note: the internal endpoint uses `token` prefix (not `Bearer`), matching how GitHub's
own extensions call it. However, `Bearer gho_xxx` also works in practice.

Additional headers that VS Code / copilot-api send (mimic these for reliability):

```
Content-Type: application/json
Accept: application/json
editor-version: vscode/1.99.3
editor-plugin-version: copilot-chat/0.26.7
user-agent: GitHubCopilotChat/0.26.7
x-github-api-version: 2025-04-01
x-vscode-user-agent-library-version: electron-fetch
```

The thin plugin already constructs equivalent headers (see `_build_extra_headers` in
`src/thin_plugins/provider_github_copilot/plugin.py`). The usage call can reuse the
same `_oauth_token` and header-building logic.

### For the official billing endpoint

```
Authorization: Bearer <PAT or OAuth token>
Accept: application/vnd.github+json
X-GitHub-Api-Version: 2026-03-10
```

Requires a PAT with billing-related scopes. The docs do not explicitly enumerate the
required scope for the user-level endpoint, but org-level Copilot billing endpoints
require `manage_billing:copilot` or `read:org`. For user-level, a token with `read:user`
and `copilot` scopes should suffice (unverified — test empirically).

## Response shape

### Internal endpoint (`/copilot_internal/user`)

```json
{
  "access_type_sku": "plus_monthly_subscriber_quota",
  "analytics_tracking_id": "uuid-string",
  "assigned_date": "2025-01-15",
  "can_signup_for_limited": false,
  "chat_enabled": true,
  "copilot_plan": "individual_pro",
  "organization_login_list": [],
  "organization_list": [],
  "quota_reset_date": "2026-05-01",
  "quota_snapshots": {
    "chat": {
      "entitlement": 0,
      "overage_count": 0,
      "overage_permitted": false,
      "percent_remaining": 100.0,
      "quota_id": "chat",
      "quota_remaining": 0,
      "remaining": 0,
      "unlimited": true
    },
    "completions": {
      "entitlement": 0,
      "overage_count": 0,
      "overage_permitted": false,
      "percent_remaining": 100.0,
      "quota_id": "completions",
      "quota_remaining": 0,
      "remaining": 0,
      "unlimited": true
    },
    "premium_interactions": {
      "entitlement": 300,
      "overage_count": 0,
      "overage_permitted": false,
      "percent_remaining": 31.17,
      "quota_id": "premium_interactions",
      "quota_remaining": 93.5,
      "remaining": 93,
      "unlimited": false
    }
  }
}
```

Key fields for a plugin:

| Field | Type | Meaning |
|-------|------|---------|
| `copilot_plan` | string | `"individual_free"`, `"individual_pro"`, `"individual_pro_plus"`, `"business"`, `"enterprise"` |
| `quota_reset_date` | string | ISO date (YYYY-MM-DD), always 1st of next month |
| `quota_snapshots.premium_interactions.entitlement` | number | Total premium requests in cycle (e.g. 300 for Pro) |
| `quota_snapshots.premium_interactions.remaining` | integer | Remaining (truncated) |
| `quota_snapshots.premium_interactions.quota_remaining` | float | Remaining (precise, fractional due to sub-1x multipliers) |
| `quota_snapshots.premium_interactions.percent_remaining` | float | 0-100 |
| `quota_snapshots.premium_interactions.overage_permitted` | bool | Whether pay-per-use overflow is enabled |
| `quota_snapshots.premium_interactions.overage_count` | number | Overages consumed so far |
| `quota_snapshots.premium_interactions.unlimited` | bool | True for chat/completions on paid plans |
| `quota_snapshots.chat.unlimited` | bool | True on paid plans |
| `quota_snapshots.completions.unlimited` | bool | True on paid plans |

### Official billing endpoint

```json
{
  "timePeriod": { "year": 2026, "month": 4 },
  "user": "octocat",
  "usageItems": [
    {
      "product": "copilot",
      "sku": "premium_requests",
      "model": "claude-sonnet-4",
      "unitType": "premium_request",
      "pricePerUnit": 0.04,
      "grossQuantity": 15,
      "grossAmount": 0.60,
      "discountQuantity": 15,
      "discountAmount": 0.60,
      "netQuantity": 0,
      "netAmount": 0.00
    }
  ]
}
```

This endpoint reports spend, not remaining quota. Discount items represent the included
allowance. `netAmount` is the out-of-pocket cost after allowance.

## Reset schedule

Premium request counters reset on the **1st of each month at 00:00:00 UTC**, regardless
of the user's signup date or billing cycle. The `quota_reset_date` field in the internal
endpoint response confirms the next reset date.

Unused requests do **not** carry over to the following month.

## Premium request allowances by plan

| Plan | Monthly premium requests | Included models (0x cost) |
|------|--------------------------|---------------------------|
| Free | 50 | GPT-4.1, GPT-4o, GPT-5 mini |
| Pro ($10/mo) | 300 | GPT-4.1, GPT-4o, GPT-5 mini |
| Pro+ ($39/mo) | 1,500 | GPT-4.1, GPT-4o, GPT-5 mini |
| Business | 300 per user | GPT-4.1, GPT-4o, GPT-5 mini |
| Enterprise | 1,000 per user | GPT-4.1, GPT-4o, GPT-5 mini |

Overage rate: **$0.04 USD per premium request** (must be opted in).

### Model multipliers (selected)

| Model | Multiplier | Premium cost per interaction |
|-------|------------|----------------------------|
| Claude Haiku 4.5 | 0.33x | 0.33 premium requests |
| Gemini 3 Flash | 0.33x | 0.33 premium requests |
| GPT-5.4 mini | 0.33x | 0.33 premium requests |
| Grok Code Fast 1 | 0.25x | 0.25 premium requests |
| Claude Sonnet 4 / 4.5 / 4.6 | 1x | 1 premium request |
| Gemini 2.5 Pro / 3.1 Pro | 1x | 1 premium request |
| GPT-5.1 / 5.2 / 5.4 | 1x | 1 premium request |
| Claude Opus 4.5 / 4.6 | 3x | 3 premium requests |
| Claude Opus 4.6 (fast mode) | 30x | 30 premium requests |

Full table: https://docs.github.com/en/copilot/concepts/billing/copilot-requests

## Rate-limit headers (`api.githubcopilot.com`)

The chat completions endpoint (`POST /chat/completions`) returns OpenAI-style rate-limit
headers on every response:

```
x-ratelimit-remaining-tokens: 19999342
x-ratelimit-remaining-requests: 199998
x-ratelimit-limit-tokens: <not confirmed>
x-ratelimit-limit-requests: <not confirmed>
```

These represent **short-window burst limits** (per-minute or similar), not the monthly
premium request budget. They vary by auth method and plan tier. Observed values:

- OAuth device token: ~20M remaining tokens, ~200K remaining requests
- PAT: ~2M remaining tokens, ~20K remaining requests

When exceeded, responses return HTTP 429 with `"code": "rate_limited"`.

These headers are a useful signal for back-off but do **not** replace the monthly premium
request quota tracked by `/copilot_internal/user`.

## How official clients (VS Code / gh CLI) query it

### VS Code Copilot extension

The VS Code extension calls `GET https://api.github.com/copilot_internal/user`
periodically (observable via VS Code DevTools Network tab). The response populates the
Copilot status bar badge showing "X% premium requests remaining". Auth uses the
extension's OAuth session token with `Authorization: token <gho_...>`.

### ericc-ch/copilot-api (community proxy)

Source: `src/services/github/get-copilot-usage.ts`

```typescript
const response = await fetch(`${GITHUB_API_BASE_URL}/copilot_internal/user`, {
  headers: githubHeaders(state),  // authorization: `token ${state.githubToken}`
})
```

Headers constructed in `src/lib/api-config.ts`:
```typescript
export const githubHeaders = (state: State) => ({
  "content-type": "application/json",
  "accept": "application/json",
  authorization: `token ${state.githubToken}`,
  "editor-version": `vscode/${state.vsCodeVersion}`,
  "editor-plugin-version": `copilot-chat/0.26.7`,
  "user-agent": "GitHubCopilotChat/0.26.7",
  "x-github-api-version": "2025-04-01",
})
```

Exposes `/usage` HTTP route and `check-usage` CLI command for terminal display.

### gh CLI / gh-copilot extension

The `gh copilot` extension (github/gh-copilot) is closed-source. It does not appear to
expose usage/quota info directly. The `gh` CLI itself does not have a `gh copilot usage`
subcommand. However, the official billing endpoint can be queried via:

```bash
gh api /users/{username}/settings/billing/premium_request/usage \
  --header "X-GitHub-Api-Version: 2026-03-10"
```

### Zed editor

Zed implemented usage tracking via `copilot_internal/user` in PR #48419 after community
discovery in Discussion #44499.

## Implementation plan for thin plugin

The plugin should call the **internal endpoint** (`/copilot_internal/user`) since it:
- Returns real-time remaining quota (not just historical spend)
- Uses the same `gho_*` OAuth token already resolved by `provider_github_copilot`
- Is the proven approach used by VS Code, Zed, copilot-api, and LiteLLM

Sketch:

```python
async def get_usage(self) -> dict:
    """Query Copilot quota via internal endpoint."""
    import httpx
    headers = {
        "authorization": f"token {self._oauth_token}",
        "accept": "application/json",
        "content-type": "application/json",
        "editor-version": self._editor_version,
        "editor-plugin-version": self._editor_plugin_version,
        "user-agent": self._user_agent,
        "x-github-api-version": self._api_version,
    }
    async with httpx.AsyncClient(timeout=30.0) as client:
        resp = await client.get(
            "https://api.github.com/copilot_internal/user",
            headers=headers,
        )
        resp.raise_for_status()
        data = resp.json()

    snap = data.get("quota_snapshots", {}).get("premium_interactions", {})
    return {
        "plan": data.get("copilot_plan"),
        "entitlement": snap.get("entitlement", 0),
        "remaining": snap.get("remaining", 0),
        "remaining_precise": snap.get("quota_remaining", 0.0),
        "percent_remaining": snap.get("percent_remaining", 0.0),
        "overage_permitted": snap.get("overage_permitted", False),
        "overage_count": snap.get("overage_count", 0),
        "unlimited": snap.get("unlimited", False),
        "reset_date": data.get("quota_reset_date"),
    }
```

## Open questions / unknowns

1. **Auth scope for `/copilot_internal/user`**: Works with `gho_*` tokens that have
   `read:user` scope (the default for Copilot OAuth apps). Unclear if a classic PAT with
   `copilot` scope also works — needs empirical testing.

2. **Stability of internal endpoint**: `/copilot_internal/user` is undocumented. GitHub
   could change or remove it. No versioning guarantee. However, it has been stable for
   over a year and is depended on by VS Code itself.

3. **Enterprise/Business base URL**: For org-managed Copilot seats, the chat endpoint
   uses `api.{accountType}.githubcopilot.com` but the internal user endpoint may still
   use `api.github.com`. Untested for enterprise accounts.

4. **Official billing endpoint scopes**: The docs do not explicitly state which PAT
   scopes are required for `GET /users/{username}/settings/billing/premium_request/usage`.
   Likely needs `read:user` or a billing-related scope.

5. **Rate-limit headers for `x-ratelimit-limit-*`**: Only `remaining` variants have been
   confirmed in community reports. The `limit` counterparts (total budget for the
   window) have not been explicitly documented.

6. **Quota snapshot freshness**: How frequently the server updates `quota_snapshots` is
   unknown. It appears near-real-time but could have a short cache TTL.

## Sources

- [GitHub Docs: Requests in GitHub Copilot](https://docs.github.com/en/copilot/concepts/billing/copilot-requests) — model multipliers, included models
- [GitHub Docs: Billing usage REST API](https://docs.github.com/en/rest/billing/usage?apiVersion=2026-03-10) — official `/users/{username}/settings/billing/premium_request/usage` endpoint
- [GitHub Docs: Copilot premium requests](https://docs.github.com/en/billing/concepts/product-billing/github-copilot-premium-requests) — overage pricing, allowance info
- [GitHub Docs: Plans for GitHub Copilot](https://docs.github.com/en/copilot/get-started/plans) — plan quotas (50/300/1500/300/1000)
- [GitHub Docs: Rate limits for GitHub Copilot](https://docs.github.com/en/copilot/concepts/rate-limits) — burst rate-limit docs
- [GitHub Docs: Monitoring your GitHub Copilot usage](https://docs.github.com/copilot/how-tos/monitoring-your-copilot-usage-and-entitlements) — official monitoring guidance
- [ericc-ch/copilot-api source](https://github.com/ericc-ch/copilot-api) — reverse-engineered `/copilot_internal/user` call (src/services/github/get-copilot-usage.ts, src/lib/api-config.ts)
- [Zed Discussion #44499](https://github.com/zed-industries/zed/discussions/44499) — community-verified curl with full response body
- [LiteLLM Issue #18242](https://github.com/BerriAI/litellm/issues/18242) — confirmed response shape with example data
- [GitHub Community Discussion #157693](https://github.com/orgs/community/discussions/157693) — per-user usage API gap discussion
- [GitHub Community Discussion #138918](https://github.com/orgs/community/discussions/138918) — x-ratelimit header values reported
- [GitHub Community Discussion #184208](https://github.com/orgs/community/discussions/184208) — enterprise per-user usage API gap
