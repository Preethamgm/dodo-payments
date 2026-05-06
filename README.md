# Dodo Payments — Invoice & Payment Service

A minimal Invoice & Payment Service built in Rust (Axum + PostgreSQL). Businesses create invoices for customers, customers pay invoices, and the business is notified via signed webhooks.

---

## Demo Video

> https://www.loom.com/share/cc0198b8f5df4693991a2b709e330fc4 (Adjust the volume)

---

## Tech Stack

- **Language:** Rust (Axum web framework)
- **Database:** PostgreSQL 15
- **Async runtime:** Tokio
- **ORM/Query:** sqlx
- **Auth:** API key (SHA-256 hashed, Bearer token)
- **Webhooks:** HMAC-SHA256 signed, async delivery with exponential backoff

---

## Running the Project

### Prerequisites
- Docker Desktop (running)
- Rust 1.75+ (`rustup`)

### One-command setup

```bash
docker compose up --build
```

This starts:
- **PostgreSQL** on port 5432
- **Mock PSP** on port 3001
- **Invoice Service** on port 3000

Migrations run automatically on startup. A test business and API key are printed to the server logs on first boot:

```
═══════════════════════════════════════
  TEST API KEY: sk_xxxxxxxxxxxxxx
  Save this! It won't be shown again.
═══════════════════════════════════════
```

Copy that key — you'll need it for all API calls.

---

## curl Examples

Replace `YOUR_API_KEY` with the key printed at startup.

### 1. Create a customer

```bash
curl -X POST http://localhost:3000/customers \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name": "Jane Smith", "email": "jane@example.com"}'
```

**Response:**
```json
{
  "id": "d43ed356-a611-468e-be0f-7dab06f2332b",
  "name": "Jane Smith",
  "email": "jane@example.com",
  "created_at": "2026-05-05T04:14:45.640112Z"
}
```

---

### 2. Create an invoice

```bash
curl -X POST http://localhost:3000/invoices \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "customer_id": "d43ed356-a611-468e-be0f-7dab06f2332b",
    "due_date": "2026-06-01",
    "line_items": [
      {"description": "Website Design", "quantity": 1, "unit_amount_cents": 50000},
      {"description": "Hosting", "quantity": 12, "unit_amount_cents": 1000}
    ]
  }'
```

**Response:**
```json
{
  "id": "507fc98f-30af-4887-bf1c-a4427dd416cc",
  "customer_id": "d43ed356-a611-468e-be0f-7dab06f2332b",
  "state": "open",
  "total_cents": 62000,
  "due_date": "2026-06-01",
  "line_items": [
    {"description": "Website Design", "quantity": 1, "unit_amount_cents": 50000, "total_cents": 50000},
    {"description": "Hosting", "quantity": 12, "unit_amount_cents": 1000, "total_cents": 12000}
  ],
  "created_at": "2026-05-05T04:17:06Z"
}
```

---

### 3. Pay an invoice — success

```bash
curl -X POST http://localhost:3000/invoices/507fc98f-30af-4887-bf1c-a4427dd416cc/pay \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -H "Idempotency-Key: unique-key-001" \
  -d '{"card_token": "tok_success"}'
```

**Response:**
```json
{
  "payment_attempt_id": "0e2279c7-701f-465c-9f51-10a3dc92e478",
  "status": "succeeded",
  "psp_ref": "3d5e7337-2a93-4999-bf49-0a698c096892",
  "failure_code": null
}
```

---

### 4. Pay an invoice — card declined

```bash
curl -X POST http://localhost:3000/invoices/INVOICE_ID/pay \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -H "Idempotency-Key: unique-key-002" \
  -d '{"card_token": "tok_card_declined"}'
```

**Response:**
```json
{
  "payment_attempt_id": "73a66058-d004-492e-a6ca-9d665611fecf",
  "status": "failed",
  "psp_ref": null,
  "failure_code": "card_declined"
}
```

---

## Mock PSP Card Tokens

| Token | Behavior |
|-------|----------|
| `tok_success` | Returns succeeded after ~100ms |
| `tok_insufficient_funds` | Returns failed, code: insufficient_funds |
| `tok_card_declined` | Returns failed, code: card_declined |
| `tok_timeout` | Sleeps 30s — service times out after 10s, returns pending |
| `tok_network_error` | Drops connection — service returns failed |

---

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| POST | /customers | Create a customer |
| GET | /customers | List customers |
| GET | /customers/:id | Get a customer |
| POST | /invoices | Create an invoice |
| GET | /invoices | List invoices (filter by ?state=) |
| GET | /invoices/:id | Get an invoice |
| POST | /invoices/:id/pay | Pay an invoice |
| POST | /webhooks | Register a webhook endpoint |

---

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `DATABASE_URL` | — | Postgres connection string |
| `PSP_URL` | http://localhost:3001 | Mock PSP base URL |
| `WEBHOOK_SECRET` | — | Secret for HMAC-SHA256 webhook signing |

---

## Running Tests

```bash
cargo test
```

Tests cover:
- Concurrent payment attempts (no double charge)
- Idempotency key reuse
- PSP timeout handling

---

## Project Structure

```
dodo-payments/
├── src/
│   ├── main.rs           # App entry point, routing, seeding
│   ├── mock_psp.rs       # Mock payment processor service
│   ├── db.rs             # All database queries
│   ├── models.rs         # Data types and enums
│   ├── errors.rs         # Unified error type + HTTP responses
│   ├── webhooks.rs       # Webhook delivery + signing + retry
│   └── handlers/
│       ├── mod.rs
│       ├── auth.rs       # API key middleware
│       ├── customers.rs  # Customer endpoints
│       ├── invoices.rs   # Invoice endpoints
│       ├── payments.rs   # Payment endpoint
│       └── webhooks.rs   # Webhook registration endpoint
├── migrations/           # SQL migration files
├── docker-compose.yml
├── Dockerfile
├── DESIGN.md
├── AI_USAGE.md
└── openapi.yaml
```
