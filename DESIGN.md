# DESIGN.md — Dodo Payments Invoice & Payment Service

## 1. Data Model

### Tables

**businesses**
- `id` UUID PK
- `name` TEXT
- `email` TEXT UNIQUE
- `created_at` TIMESTAMPTZ

**api_keys**
- `id` UUID PK
- `business_id` UUID FK → businesses
- `key_hash` TEXT UNIQUE (SHA-256 hash of the raw key)
- `key_prefix` TEXT (first 7 chars, e.g. `sk_5vJ2n`, for display/identification)
- `created_at` TIMESTAMPTZ
- `revoked_at` TIMESTAMPTZ (NULL = active)

**customers**
- `id` UUID PK
- `business_id` UUID FK → businesses
- `name` TEXT
- `email` TEXT
- UNIQUE(business_id, email) — same email can exist across businesses

**invoices**
- `id` UUID PK
- `business_id` UUID FK → businesses
- `customer_id` UUID FK → customers
- `state` invoice_state ENUM
- `total_cents` BIGINT — integer cents only, no floats
- `due_date` DATE
- `created_at`, `updated_at` TIMESTAMPTZ

**line_items**
- `id` UUID PK
- `invoice_id` UUID FK → invoices
- `description` TEXT
- `quantity` INTEGER (> 0)
- `unit_amount_cents` BIGINT (> 0)
- `total_cents` BIGINT = quantity × unit_amount_cents

**payment_attempts**
- `id` UUID PK
- `invoice_id` UUID FK → invoices
- `idempotency_key` TEXT UNIQUE
- `status` payment_status ENUM (pending/succeeded/failed)
- `card_token` TEXT
- `psp_ref` TEXT (returned by PSP on success)
- `failure_code` TEXT (returned by PSP on failure)
- `request_hash` SHA-256 of (invoice_id + card_token) — used to detect same-key-different-body
- `created_at`, `updated_at` TIMESTAMPTZ

**webhook_endpoints**
- `id` UUID PK
- `business_id` UUID FK → businesses
- `url` TEXT
- `created_at` TIMESTAMPTZ

**webhook_deliveries**
- `id` UUID PK
- `endpoint_id` UUID FK → webhook_endpoints
- `event_type` TEXT (invoice.created, invoice.paid, invoice.payment_failed)
- `payload` JSONB
- `status` webhook_status ENUM (pending/delivered/failed)
- `attempts` INTEGER
- `next_retry_at` TIMESTAMPTZ
- `delivered_at` TIMESTAMPTZ

### Indexes
- `api_keys(key_hash)` — fast auth lookup on every request
- `invoices(business_id)`, `invoices(state)` — list filtering
- `payment_attempts(idempotency_key)` — fast idempotency check
- `webhook_deliveries(status, next_retry_at)` — retry queue polling

### Why UUIDs over sequential IDs?
UUIDs are safe to expose in URLs (no enumeration attacks), work across distributed systems, and can be generated client-side if needed.

### Money
All monetary values are stored as BIGINT in cents. No NUMERIC, no FLOAT, no DECIMAL anywhere in the money path. The server always computes totals from line items — client-supplied totals are ignored entirely.

### At 100x scale
- Partition `invoices` and `payment_attempts` by `business_id` or date range
- Move webhook delivery to a dedicated queue (Redis/SQS) instead of polling the DB
- Add read replicas for list queries
- Index `invoices(business_id, created_at DESC)` for pagination

---

## 2. Invoice State Machine

```
         ┌─────────┐
         │  draft  │
         └────┬────┘
              │ (invoice created)
              ▼
         ┌─────────┐
    ┌───▶│  open   │◀──────────────┐
    │    └────┬────┘               │
    │         │ POST /pay          │
    │         │                    │ PSP fails
    │         ▼                    │
    │   [PSP called]───────────────┘
    │         │ PSP succeeds
    │         ▼
    │    ┌─────────┐
    │    │  paid   │  ← TERMINAL
    │    └─────────┘
    │
    │    ┌──────────────┐
    └────│ uncollectible│  ← TERMINAL (manual/admin action)
         └──────────────┘

         ┌──────┐
         │ void │  ← TERMINAL (manual/admin action)
         └──────┘
```

### Valid Transitions

| From | To | Trigger |
|------|----|---------|
| draft | open | Invoice created (automatic) |
| open | paid | PSP returns success |
| open | open | PSP returns failure (stays open, retryable) |
| open | void | Business voids invoice (admin action) |
| open | uncollectible | Business marks uncollectible (admin action) |

### Terminal States
`paid`, `void`, `uncollectible` — no further transitions allowed from these states.

### Invalid Transition Rejection
The payment handler acquires a `SELECT FOR UPDATE` row lock on the invoice, reads the current state, and returns HTTP 422 with a descriptive error if the transition is not allowed. This happens before any PSP call is made, so no charge is attempted on an invalid transition.

---

## 3. Payment Correctness & Failure Modes

### (a) Two concurrent POST /pay for the same invoice

Both requests hit the handler simultaneously. After the idempotency check, the handler runs:

```sql
SELECT id, state FROM invoices WHERE id = $1 AND business_id = $2 FOR UPDATE
```

`FOR UPDATE` acquires a row-level exclusive lock. The second request blocks at this line until the first completes and commits. By the time the second request acquires the lock, the invoice state is already `paid`, so it receives HTTP 422 `INVALID_STATE_TRANSITION`. No double charge occurs. The mechanism is Postgres row-level locking.

### (b) PSP timeout (tok_timeout, 30s)

The PSP call is wrapped in a 10-second timeout:

```rust
tokio::time::timeout(Duration::from_secs(10), psp_client.post(...).send()).await
```

After 10 seconds with no response, the timeout fires. The payment attempt is saved with `status = pending` and `failure_code = psp_timeout`. The invoice stays `open`. The API returns immediately to the caller with `status: pending`. The caller can poll `GET /invoices/{id}` to check the eventual outcome. In production, a background reconciliation job would query the PSP for the outcome using the PSP reference.

### (c) PSP returns success but service crashes before persisting

On retry with the same idempotency key, the handler finds no existing payment attempt record (it was never saved before the crash). It calls the PSP again with a new request. Whether the customer is double-charged depends on whether the PSP supports idempotency keys — in production we would pass our `payment_attempt.id` as the PSP-side idempotency key so the PSP deduplicates on their end and returns the original result. This is a known production gap documented in Section 7.

### (d) Idempotency key reused with a different request body

The handler computes SHA-256 of `(invoice_id + card_token)` and stores it as `request_hash` alongside the payment attempt. On retry, if the idempotency key exists but the computed hash does not match the stored one, the handler returns HTTP 409 Conflict:

```json
{"error": {"code": "CONFLICT", "message": "Idempotency key reused with different request body"}}
```

The original payment attempt is not modified.

### (e) POST /pay on an already-paid invoice

The `FOR UPDATE` lock is acquired and the state is read as `paid`. The handler immediately returns HTTP 422 before making any PSP call:

```json
{"error": {"code": "INVALID_STATE_TRANSITION", "message": "Invoice is already paid"}}
```

### Concurrency Mechanism: Row-level lock (SELECT FOR UPDATE)

**Why not optimistic concurrency?** Requires retry logic on the client side. Poor UX for a payment endpoint where the caller expects a definitive answer.

**Why not advisory locks?** More complex to manage, require explicit lock/unlock pairs, and are harder to reason about in failure scenarios.

**Why not serializable isolation?** Adds overhead to every transaction in the system, not just payment ones. Overkill for this use case.

Row-level locking serializes payment attempts per invoice with minimal overhead and is the simplest correct mechanism for this pattern.

---

## 4. Webhook Design

### Signing Scheme
Each webhook delivery is signed with HMAC-SHA256:

```
X-Dodo-Signature: hex(HMAC-SHA256(WEBHOOK_SECRET, payload_json_string))
X-Dodo-Event: invoice.paid
```

The receiver verifies by computing the same HMAC using the shared secret and comparing with the header value. The secret is shared out-of-band at endpoint registration time.

**Replay protection:** In production, a `webhook_timestamp` (Unix epoch) would be included in the payload and signed alongside the body. Receivers would reject webhooks with a timestamp older than 5 minutes. Not implemented in this version — documented as a gap in Section 7.

### Retry Policy

| Attempt | Delay after failure |
|---------|-------------------|
| 1st retry | 10 seconds |
| 2nd retry | 30 seconds |
| 3rd retry | 60 seconds |
| 4th retry | 5 minutes |
| 5th retry | 10 minutes |

Max 5 attempts total. Total time budget: ~16 minutes. Each attempt has a 5-second HTTP timeout.

### After Retry Budget Exhausted
The `webhook_deliveries` row is marked `status = failed` and remains in the database. Businesses can reconcile missed events by calling `GET /invoices/{id}` to check current state. In production, we would expose a `GET /webhooks/deliveries` endpoint filtered by status, and send an alert to the business.

### Why Decoupled from API Response?
Webhook delivery is spawned as a background Tokio task immediately after the API operation completes:

```rust
tokio::spawn(async move { send_webhook(...).await; });
```

This means the API response returns to the caller without waiting for webhook delivery. If delivery were synchronous, a slow or unreachable webhook endpoint would directly delay the API response, harming the caller's experience and violating the principle that the webhook consumer should not affect the producer.

---

## 5. API Key Model

### Generation
Keys are generated as `sk_` followed by 32 random alphanumeric characters using a cryptographically secure RNG (`rand::thread_rng` backed by the OS CSPRNG). Total entropy: ~190 bits. Example: `sk_5vJ2nbLzLioO8911Z2tEPDDAIs3UQjVD`

### Storage
The raw key is **never stored**. Only a SHA-256 hash is stored in `api_keys.key_hash`. The first 7 characters are stored as `key_prefix` for identification purposes (e.g. showing the user which key is which in a dashboard). The raw key is shown exactly once at creation time and cannot be recovered afterward.

### Transmission
Keys are sent via `Authorization: Bearer <key>` header over HTTPS in production. On every request, the handler hashes the incoming key and does a single indexed lookup:

```sql
SELECT ... WHERE key_hash = $1 AND revoked_at IS NULL
```

### Rotation
Create a new key → update integrations → revoke the old key by setting `revoked_at = NOW()`. Both keys work during the transition window, enabling zero-downtime rotation.

### Revocation
Setting `revoked_at` on an `api_keys` row immediately invalidates it with no application restart required. The auth middleware filters revoked keys on every request.

### Blast Radius if Leaked
An attacker with a leaked key can read and modify all data scoped to that business: customers, invoices, payment attempts, webhook endpoints. They cannot access other businesses' data (all queries are scoped by `business_id`). Mitigation: revoke immediately, rotate to a new key, audit recent API call logs for suspicious activity.

---

## 6. What Was Cut and Why

**Void/uncollectible API endpoints** — The state machine and database fully support `void` and `uncollectible` states, but there are no HTTP endpoints to trigger these transitions. Would add `POST /invoices/{id}/void` and `POST /invoices/{id}/mark-uncollectible`. Cut to stay within the time budget; the state machine design is already correct.

**Refunds and partial payments** — Out of scope per the assignment spec. Would require a `refunds` table, a `refund_attempts` table, reverse PSP calls, and new invoice states (`partially_refunded`, `fully_refunded`). The current data model can accommodate this without breaking changes.

**Webhook delivery log endpoint** — Businesses currently have no API to query past webhook deliveries. Would add `GET /webhooks/deliveries?status=failed` for reconciliation. The data is already stored in `webhook_deliveries`; only the handler is missing.

**PSP-side idempotency keys** — We do not pass our `payment_attempt.id` to the mock PSP as an idempotency key. In production this is critical: without it, a crash-and-retry scenario can result in a double charge. Cut because the mock PSP does not support this, but documented as the top production gap.

**Rate limiting** — No per-key rate limiting is implemented. In production: sliding window counters in Redis, enforced at the reverse proxy layer (nginx/Cloudflare) before requests reach the app. Discussed in Section 7.

---

## 7. Production Readiness Gaps

**1. Observability**
No metrics, distributed tracing, or structured log aggregation. The `tracing` crate is wired up but only writes to stdout. In production: add Prometheus metrics (request latency p99, payment success rate, webhook delivery rate, PSP timeout rate), OpenTelemetry tracing with a Jaeger or Tempo exporter, and ship structured JSON logs to Datadog or Grafana Loki. Without this, debugging production incidents is nearly impossible.

**2. PSP Idempotency and Crash Recovery**
If the service crashes after the PSP returns success but before the result is persisted to the database, a retry will call the PSP again and may double-charge the customer. The fix is to pass `payment_attempt.id` as the PSP-side idempotency key on every charge request. The PSP then deduplicates and returns the original result on retry. This is the single most important correctness gap for a real payments system.

**3. Webhook Replay Protection**
Current webhook signatures have no timestamp component, so a captured payload could be replayed indefinitely by an attacker who intercepts it. The fix is to include a `dodo_timestamp` Unix timestamp in every webhook payload, sign it as part of the HMAC input, and have receivers reject webhooks where `abs(now - dodo_timestamp) > 300 seconds`. This is standard practice (used by Stripe, Svix, etc.).
