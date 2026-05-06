use sqlx::PgPool;
use uuid::Uuid;
use crate::models::*;
use chrono::NaiveDate;

// ── Businesses ──────────────────────────────────────────────────────────────

pub async fn get_business_by_api_key(pool: &PgPool, key_hash: &str) -> Option<Business> {
    sqlx::query_as!(
        Business,
        r#"
        SELECT b.id, b.name, b.email, b.created_at
        FROM businesses b
        JOIN api_keys k ON k.business_id = b.id
        WHERE k.key_hash = $1 AND k.revoked_at IS NULL
        "#,
        key_hash
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

// ── Customers ────────────────────────────────────────────────────────────────

pub async fn create_customer(
    pool: &PgPool,
    business_id: Uuid,
    name: &str,
    email: &str,
) -> Result<Customer, sqlx::Error> {
    sqlx::query_as!(
        Customer,
        r#"
        INSERT INTO customers (business_id, name, email)
        VALUES ($1, $2, $3)
        RETURNING *
        "#,
        business_id,
        name,
        email
    )
    .fetch_one(pool)
    .await
}

pub async fn get_customer(
    pool: &PgPool,
    business_id: Uuid,
    customer_id: Uuid,
) -> Option<Customer> {
    sqlx::query_as!(
        Customer,
        "SELECT * FROM customers WHERE id = $1 AND business_id = $2",
        customer_id,
        business_id
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

pub async fn list_customers(pool: &PgPool, business_id: Uuid) -> Vec<Customer> {
    sqlx::query_as!(
        Customer,
        "SELECT * FROM customers WHERE business_id = $1 ORDER BY created_at DESC",
        business_id
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

// ── Invoices ─────────────────────────────────────────────────────────────────

pub async fn create_invoice(
    pool: &PgPool,
    business_id: Uuid,
    customer_id: Uuid,
    due_date: NaiveDate,
    line_items: &[(String, i32, i64)],
) -> Result<Invoice, sqlx::Error> {
    let total_cents: i64 = line_items
        .iter()
        .map(|(_, qty, unit)| (*qty as i64) * unit)
        .sum();

    let mut tx = pool.begin().await?;

    let invoice = sqlx::query_as!(
        Invoice,
        r#"
        INSERT INTO invoices (business_id, customer_id, state, total_cents, due_date)
        VALUES ($1, $2, 'open', $3, $4)
        RETURNING id, business_id, customer_id, state AS "state: InvoiceState",
                  total_cents, due_date, created_at, updated_at
        "#,
        business_id,
        customer_id,
        total_cents,
        due_date
    )
    .fetch_one(&mut *tx)
    .await?;

    for (desc, qty, unit_amount) in line_items {
        let line_total = (*qty as i64) * unit_amount;
        sqlx::query!(
            r#"
            INSERT INTO line_items (invoice_id, description, quantity, unit_amount_cents, total_cents)
            VALUES ($1, $2, $3, $4, $5)
            "#,
            invoice.id,
            desc,
            qty,
            unit_amount,
            line_total
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(invoice)
}

pub async fn get_invoice(
    pool: &PgPool,
    business_id: Uuid,
    invoice_id: Uuid,
) -> Option<Invoice> {
    sqlx::query_as!(
        Invoice,
        r#"
        SELECT id, business_id, customer_id, state AS "state: InvoiceState",
               total_cents, due_date, created_at, updated_at
        FROM invoices
        WHERE id = $1 AND business_id = $2
        "#,
        invoice_id,
        business_id
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

pub async fn list_invoices(
    pool: &PgPool,
    business_id: Uuid,
    state: Option<InvoiceState>,
) -> Vec<Invoice> {
    match state {
        Some(s) => sqlx::query_as!(
            Invoice,
            r#"
            SELECT id, business_id, customer_id, state AS "state: InvoiceState",
                   total_cents, due_date, created_at, updated_at
            FROM invoices
            WHERE business_id = $1 AND state = $2
            ORDER BY created_at DESC
            "#,
            business_id,
            s as InvoiceState
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default(),
        None => sqlx::query_as!(
            Invoice,
            r#"
            SELECT id, business_id, customer_id, state AS "state: InvoiceState",
                   total_cents, due_date, created_at, updated_at
            FROM invoices
            WHERE business_id = $1
            ORDER BY created_at DESC
            "#,
            business_id
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default(),
    }
}

pub async fn get_line_items(pool: &PgPool, invoice_id: Uuid) -> Vec<LineItem> {
    sqlx::query_as!(
        LineItem,
        "SELECT * FROM line_items WHERE invoice_id = $1",
        invoice_id
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

// ── Payment Attempts ──────────────────────────────────────────────────────────

pub async fn get_payment_attempt_by_idempotency_key(
    pool: &PgPool,
    key: &str,
) -> Option<PaymentAttempt> {
    sqlx::query_as!(
        PaymentAttempt,
        r#"
        SELECT id, invoice_id, idempotency_key, status AS "status: PaymentStatus",
               card_token, psp_ref, failure_code, request_hash, created_at, updated_at
        FROM payment_attempts WHERE idempotency_key = $1
        "#,
        key
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

pub async fn create_payment_attempt(
    pool: &PgPool,
    invoice_id: Uuid,
    idempotency_key: &str,
    card_token: &str,
    request_hash: &str,
) -> Result<PaymentAttempt, sqlx::Error> {
    sqlx::query_as!(
        PaymentAttempt,
        r#"
        INSERT INTO payment_attempts
            (invoice_id, idempotency_key, status, card_token, request_hash)
        VALUES ($1, $2, 'pending', $3, $4)
        RETURNING id, invoice_id, idempotency_key, status AS "status: PaymentStatus",
                  card_token, psp_ref, failure_code, request_hash, created_at, updated_at
        "#,
        invoice_id,
        idempotency_key,
        card_token,
        request_hash
    )
    .fetch_one(pool)
    .await
}

pub async fn update_payment_attempt(
    pool: &PgPool,
    attempt_id: Uuid,
    status: PaymentStatus,
    psp_ref: Option<String>,
    failure_code: Option<String>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        UPDATE payment_attempts
        SET status = $2, psp_ref = $3, failure_code = $4, updated_at = NOW()
        WHERE id = $1
        "#,
        attempt_id,
        status as PaymentStatus,
        psp_ref,
        failure_code
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_invoice_state(
    pool: &PgPool,
    invoice_id: Uuid,
    new_state: InvoiceState,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        UPDATE invoices SET state = $2, updated_at = NOW() WHERE id = $1
        "#,
        invoice_id,
        new_state as InvoiceState
    )
    .execute(pool)
    .await?;
    Ok(())
}

// ── Webhooks ──────────────────────────────────────────────────────────────────

pub async fn create_webhook_endpoint(
    pool: &PgPool,
    business_id: Uuid,
    url: &str,
) -> Result<WebhookEndpoint, sqlx::Error> {
    sqlx::query_as!(
        WebhookEndpoint,
        "INSERT INTO webhook_endpoints (business_id, url) VALUES ($1, $2) RETURNING *",
        business_id,
        url
    )
    .fetch_one(pool)
    .await
}

pub async fn get_webhook_endpoints(
    pool: &PgPool,
    business_id: Uuid,
) -> Vec<WebhookEndpoint> {
    sqlx::query_as!(
        WebhookEndpoint,
        "SELECT * FROM webhook_endpoints WHERE business_id = $1",
        business_id
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

pub async fn create_webhook_delivery(
    pool: &PgPool,
    endpoint_id: Uuid,
    event_type: &str,
    payload: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO webhook_deliveries (endpoint_id, event_type, payload, next_retry_at)
        VALUES ($1, $2, $3, NOW())
        "#,
        endpoint_id,
        event_type,
        payload
    )
    .execute(pool)
    .await?;
    Ok(())
}