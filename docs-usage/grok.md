# Grok / xAI quota surface

## Auth (discovery order)

| Priority | Source | Notes |
|----------|--------|--------|
| 1 | `~/.grok/auth.json` | **Grok Build** session from `grok login` (also `$GROK_HOME/auth.json`) |
| 2 | `XAI_MANAGEMENT_KEY` / `XAI_MGMT_KEY` / `GROK_MANAGEMENT_KEY` | Management key for API prepaid balance |
| 3 | `~/.xai-management-key` etc. | Same management key as a file |
| 4 | `XAI_API_KEY` / `GROK_CODE_XAI_API_KEY` | Inference key — last resort; often cannot read billing |

Grok Build auth map shape (redacted):

```json
{
  "https://auth.x.ai::<client_id>": {
    "key": "<session JWT>",
    "expires_at": "…",
    "refresh_token": "…",
    "team_id": "…"
  }
}
```

We pick the newest non-expired `key` entry.

## Endpoints

### Primary — Grok Build (two billing shapes, both fetched)

Base: `https://cli-chat-proxy.grok.com`  
Headers: `Authorization: Bearer <session>`, `x-grok-client-version`, `x-grok-client-surface: grok-build`

`x-grok-client-version` is resolved from the installed Grok Build client (not hardcoded): `$GROK_HOME/version.json` or `~/.grok/version.json` first, then `grok --version` (~40ms) if the file is missing.

#### 1. Default monthly $ allowance

`GET /v1/billing`

```json
{
  "config": {
    "monthlyLimit": { "val": 15000 },
    "used": { "val": 2931 },
    "onDemandCap": { "val": 0 },
    "billingPeriodStart": "2026-07-01T00:00:00+00:00",
    "billingPeriodEnd": "2026-08-01T00:00:00+00:00",
    "history": [ { "billingCycle": { "year": 2026, "month": 6 }, "includedUsed": { "val": 0 }, "onDemandUsed": { "val": 0 }, "totalUsed": { "val": 0 } } ]
  }
}
```

`monthlyLimit` / `used` are **USD cents** → ×10_000 USD units in our model.

Live fixtures: `tests/fixtures/grok/cli_billing_default.json`

#### 2. Weekly product usage % (what Grok Build `/usage` shows as WEEKLY)

`GET /v1/billing?format=credits`

```json
{
  "config": {
    "currentPeriod": {
      "type": "USAGE_PERIOD_TYPE_WEEKLY",
      "start": "2026-07-07T10:46:52.885620+00:00",
      "end": "2026-07-14T10:46:52.885620+00:00"
    },
    "creditUsagePercent": 75.0,
    "productUsage": [ { "product": "GrokBuild", "usagePercent": 75.0 } ],
    "isUnifiedBillingUser": true,
    "prepaidBalance": { "val": 0 },
    "onDemandCap": { "val": 0 },
    "onDemandUsed": { "val": 0 },
    "topUpMethod": "TOP_UP_METHOD_SAVED_PAYMENT_METHOD",
    "billingPeriodStart": "…",
    "billingPeriodEnd": "…"
  }
}
```

Percentages map to `used/100` windows. `currentPeriod.type` drives the label (`weekly` / `monthly` / …).

Live fixtures: `tests/fixtures/grok/cli_billing_format_credits.json`

### Fallback — API prepaid (Management API)

Base: `https://management-api.x.ai` (requires a **management key**, not a session token)

1. `GET /auth/management-keys/validation` — resolve `teamId`
2. `GET /v1/billing/teams/{team_id}/prepaid/balance`
3. `GET /v1/billing/teams/{team_id}/postpaid/invoice/preview` (optional)

OAuth session tokens are rejected by the Management API (`oauth2-auth-forbidden`).

## Windows we surface

| Window | Path | Source |
|--------|------|--------|
| `weekly/build` (etc.) | `?format=credits` | `productUsage[].usagePercent` + `currentPeriod` |
| `weekly` | `?format=credits` | `creditUsagePercent` when no product rows |
| `monthly` | default billing | `monthlyLimit` / `used` (USD cents) |
| `on_demand_usd` | either | `onDemandCap` / `onDemandUsed` when cap > 0 |
| `balance_usd` | credits or management | prepaid remaining |
| `credits_usd` / `spend_limit_usd` / `granted_usd` | management | invoice preview fields |

## Console

- Grok usage: https://grok.com?_s=usage
- API billing: https://console.x.ai/team/default/billing
- Management keys: https://console.x.ai/team/default/settings/management-keys
