# AI Provider Support Status

Legend: `[ ]` = not supported, `~` = partial (auth/balance only), `x` = full (quota windows with rates)

Research date: 2026-04-13

---

## Tier 1 — Implemented (coding/quota plans or balance)

| Status | Provider | Notes |
|--------|----------|-------|
| `x`    | **Kimi / Moonshot** | Coding plan (weekly + 5h windows); PAYG balance fallback |
| `x`    | **MiniMax** | Rate-limit windows per model (5h + 7d); multi-model grid view |
| `x`    | **Claude (Anthropic)** | OAuth + API key; usage/quota endpoint |
| `x`    | **Codex (OpenAI)** | OAuth + API key |
| `x`    | **Z.ai (Zhipu GLM)** | Balance + rate windows |
| `~`    | **DeepSeek** | Balance only (CNY/USD); no token-quota plan endpoint yet |
| `~`    | **SiliconFlow** | Balance only (CNY paid + free); no per-model rate windows |
| `~`    | **OpenRouter** | Credits balance (USD); no per-model rate windows |

---

## Tier 2 — Known coding plans, not yet implemented

| Status | Provider | Plan / endpoint notes |
|--------|----------|-----------------------|
| `[ ]`  | **Groq** | Rate limits in response headers (`x-ratelimit-limit-tokens`, etc.) per model. Cheap/free tier has hard TPM caps. Need to make a probe request to read headers. User offered a key. |
| `[ ]`  | **GitHub Copilot** | Subscription plan (Individual / Business / Enterprise). `/copilot_internal/v2/token` used by the extension. No public usage/quota REST endpoint documented; may need to scrape. |
| `[ ]`  | **Google Gemini** | `generativelanguage.googleapis.com` — free tier has RPM/TPD limits per model surfaced as 429. No dedicated quota endpoint; rate-limit headers present on responses. Paid via Google Cloud (no simple balance endpoint). |
| `[ ]`  | **GitHub Copilot for CLI** | Subset of Copilot — same auth path, different quota surface. |

---

## Tier 3 — PAYG / balance-only providers to note

| Status | Provider | Notes |
|--------|----------|-----------------------|
| `[ ]`  | **Mistral AI** | `api.mistral.ai` — free tier (La Plateforme) with RPM/TPM rate limits; paid is PAYG. `/v1/usage` endpoint exists for billing, not real-time quota. |
| `[ ]`  | **Cohere** | `api.cohere.com` — free trial credits; PAYG after. No public quota endpoint. |
| `[ ]`  | **Together AI** | `api.together.xyz` — PAYG credits. Balance via dashboard API (undocumented). |
| `[ ]`  | **Perplexity AI** | `api.perplexity.ai` — PAYG; monthly credit bundles available. No quota endpoint. |
| `[ ]`  | **xAI (Grok)** | `api.x.ai` — PAYG + monthly free credits ($25/mo as of 2026). No public quota endpoint documented. |
| `[ ]`  | **Fireworks AI** | `api.fireworks.ai` — PAYG + free trial credits. No public quota/balance API. |
| `[ ]`  | **Alibaba Qwen (DashScope)** | `dashscope.aliyuncs.com` — CNY PAYG + free quota per model. `/api/v1/quota/me` endpoint exists. |
| `[ ]`  | **ByteDance Doubao (Ark)** | `ark.cn-beijing.volces.com` — CNY PAYG. Balance via Volcengine dashboard API. |
| `[ ]`  | **Baidu Qianfan** | `qianfan.baidubce.com` — CNY PAYG. Balance via BCE billing API. |
| `[ ]`  | **Tencent Hunyuan** | `hunyuan.tencentcloudapi.com` — CNY PAYG. Balance via Tencent Cloud billing API. |
| `[ ]`  | **Nvidia NIM** | `integrate.api.nvidia.com` — free credits + PAYG. Credits endpoint via NGC account API. |
| `[ ]`  | **Cerebras** | `api.cerebras.ai` — free tier with RPM/TPM limits; PAYG. No public quota endpoint. |
| `[ ]`  | **AI21 Labs** | `api.ai21.com` — free credits + PAYG. No public quota endpoint. |
| `[ ]`  | **Replicate** | `api.replicate.com` — PAYG per-second billing. Spend credits available via `/v1/account`. |

---

## Providers excluded / low priority

| Provider | Reason |
|----------|--------|
| **OpenAI (direct)** | Covered via Codex. |
| **Azure OpenAI** | Enterprise; quota managed per deployment in Azure portal, no simple API. |
| **AWS Bedrock** | Enterprise; quota via AWS Service Quotas API, not provider-specific. |
| **Google Vertex AI** | Enterprise; quota via GCP Quotas API. |
| **Ollama / local models** | No external quota. |
