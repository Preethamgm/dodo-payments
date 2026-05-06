# AI_USAGE.md — Honest Account of AI Assistance

## Tools Used

**Claude (Anthropic)**
Used extensively throughout the project as a pair programmer and guide. Specific uses:

- Scaffolding the initial project structure (`Cargo.toml`, folder layout, `docker-compose.yml`, `Dockerfile`)
- Drafting the initial database schema and migration SQL files, which I reviewed and adjusted
- Writing boilerplate handler code for customers, invoices, and webhooks
- Explaining Rust-specific patterns I was unfamiliar with (e.g. `sqlx::query_as!` macros, `axum` middleware, `tokio::spawn` for background tasks)
- Debugging compile errors iteratively (the `sqlx` offline mode issue, the `axum::headers` import path issue, the `webhook_status` enum cast problem)
- Drafting DESIGN.md content based on the architectural decisions made during building

---

## Three Decisions I Made Myself

**1. Using `SELECT FOR UPDATE` for concurrency instead of optimistic locking**

Claude initially suggested optimistic concurrency control (version columns + retry) as one option. I chose row-level locking (`SELECT FOR UPDATE`) instead because payment endpoints need to give a definitive answer on the first call — asking the client to retry on conflict is poor UX for a payment flow. The lock also naturally prevents double-charges without any client-side logic.

**2. Storing `request_hash` on payment attempts for idempotency body validation**

The spec said to reject idempotency key reuse with a different body, but didn't specify how. I decided to hash `(invoice_id + card_token)` with SHA-256 and store it alongside the payment attempt. This is simple, fast, and doesn't require storing the full request body. Claude had suggested storing the raw request body as JSON — I chose the hash approach because it's more storage-efficient and avoids storing potentially sensitive card token data redundantly.

**3. 10-second PSP timeout instead of the full 30 seconds**

The mock PSP's `tok_timeout` sleeps for 30 seconds. Claude's initial code used a 10-second timeout, which I kept deliberately. A 30-second timeout would hold an open HTTP connection and a database row lock for too long, degrading throughput under load. 10 seconds is already generous for a payment API. The trade-off is that legitimate slow PSP responses get cut off — acceptable given that `tok_timeout` is an error scenario, not a normal case.

---

## One Thing the AI Got Wrong

The initial `Cargo.toml` specified `axum-extra` with the `typed-header` feature and used `axum::TypedHeader` and `axum::headers` in the payments handler. This was wrong — in axum 0.7, `TypedHeader` lives in `axum_extra`, not `axum`, and the `headers` crate must be imported separately. The code Claude generated would not compile.

I identified this from the compiler error:
```
error[E0433]: cannot find `headers` in `axum`
error[E0531]: cannot find tuple struct or tuple variant `TypedHeader` in crate `axum`
```

I fixed it by changing the import to `use axum_extra::TypedHeader` and `use headers::{Header, HeaderName, HeaderValue}` and implementing the `Header` trait directly on my `IdempotencyKey` struct using the correct crate paths.
