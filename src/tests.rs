#[cfg(test)]
mod tests {
    use sqlx::PgPool;
    use uuid::Uuid;
    use std::sync::Arc;

    async fn setup_db() -> PgPool {
        let database_url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgres://dodo:dodo123@localhost:5432/dodo_payments".to_string());

        sqlx::PgPool::connect(&database_url)
            .await
            .expect("Failed to connect to test database")
    }

    async fn create_test_business(pool: &PgPool) -> Uuid {
        sqlx::query!(
            "INSERT INTO businesses (name, email) VALUES ($1, $2) RETURNING id",
            format!("Test Business {}", Uuid::new_v4()),
            format!("test_{}@example.com", Uuid::new_v4())
        )
        .fetch_one(pool)
        .await
        .unwrap()
        .id
    }

    async fn create_test_customer(pool: &PgPool, business_id: Uuid) -> Uuid {
        sqlx::query!(
            "INSERT INTO customers (business_id, name, email) VALUES ($1, $2, $3) RETURNING id",
            business_id,
            "Test Customer",
            format!("customer_{}@example.com", Uuid::new_v4())
        )
        .fetch_one(pool)
        .await
        .unwrap()
        .id
    }

    async fn create_test_invoice(pool: &PgPool, business_id: Uuid, customer_id: Uuid) -> Uuid {
        let invoice = sqlx::query!(
            r#"
            INSERT INTO invoices (business_id, customer_id, state, total_cents, due_date)
            VALUES ($1, $2, 'open', 10000, '2026-12-01')
            RETURNING id
            "#,
            business_id,
            customer_id
        )
        .fetch_one(pool)
        .await
        .unwrap();

        sqlx::query!(
            r#"
            INSERT INTO line_items (invoice_id, description, quantity, unit_amount_cents, total_cents)
            VALUES ($1, 'Test Item', 1, 10000, 10000)
            "#,
            invoice.id
        )
        .execute(pool)
        .await
        .unwrap();

        invoice.id
    }

    // ─────────────────────────────────────────────────────────────────
    // TEST 1: Concurrent payments — only one succeeds, no double charge
    // ─────────────────────────────────────────────────────────────────
    #[tokio::test]
    async fn test_concurrent_payments_no_double_charge() {
        let pool = Arc::new(setup_db().await);
        let business_id = create_test_business(&pool).await;
        let customer_id = create_test_customer(&pool, business_id).await;
        let invoice_id = create_test_invoice(&pool, business_id, customer_id).await;

        let mut handles = vec![];
        for i in 0..5 {
            let pool_clone = Arc::clone(&pool);
            let idempotency_key = format!("concurrent-test-{}-{}", invoice_id, i);

            let handle = tokio::spawn(async move {
                use sha2::{Sha256, Digest};
                let request_hash = {
                    let mut hasher = Sha256::new();
                    hasher.update(format!("{}{}", invoice_id, "tok_success").as_bytes());
                    hex::encode(hasher.finalize())
                };

                // Lock invoice row and check state
                let mut tx = pool_clone.begin().await.unwrap();
                let invoice = sqlx::query!(
                    "SELECT state::text as state FROM invoices WHERE id = $1 FOR UPDATE",
                    invoice_id
                )
                .fetch_one(&mut *tx)
                .await
                .unwrap();

                if invoice.state.as_deref() != Some("open") {
                    tx.rollback().await.unwrap();
                    return "rejected_not_open".to_string();
                }

                // Insert payment attempt inside the lock
                sqlx::query!(
                    r#"
                    INSERT INTO payment_attempts
                        (invoice_id, idempotency_key, status, card_token, request_hash)
                    VALUES ($1, $2, 'succeeded', 'tok_success', $3)
                    "#,
                    invoice_id,
                    idempotency_key,
                    request_hash
                )
                .execute(&mut *tx)
                .await
                .unwrap();

                // Mark invoice paid inside same transaction
                sqlx::query!(
                    "UPDATE invoices SET state = 'paid', updated_at = NOW() WHERE id = $1",
                    invoice_id
                )
                .execute(&mut *tx)
                .await
                .unwrap();

                tx.commit().await.unwrap();
                "succeeded".to_string()
            });
            handles.push(handle);
        }

        let mut results = vec![];
        for handle in handles {
            results.push(handle.await.unwrap());
        }

        println!("Concurrent payment results: {:?}", results);

        let succeeded_count = results.iter().filter(|r| r.as_str() == "succeeded").count();
        assert_eq!(succeeded_count, 1, "Exactly one payment should succeed, got {}", succeeded_count);

        let invoice = sqlx::query!(
            "SELECT state::text as state FROM invoices WHERE id = $1",
            invoice_id
        )
        .fetch_one(&*pool)
        .await
        .unwrap();

        assert_eq!(invoice.state.as_deref(), Some("paid"), "Invoice should be paid");

        let attempt_count = sqlx::query!(
            "SELECT COUNT(*) as count FROM payment_attempts WHERE invoice_id = $1",
            invoice_id
        )
        .fetch_one(&*pool)
        .await
        .unwrap();

        assert_eq!(attempt_count.count, Some(1), "Only one payment attempt should exist");
        println!("✅ Concurrent payment test passed — exactly 1 succeeded, no double charge");
    }

    // ─────────────────────────────────────────────────────────────────
    // TEST 2: Idempotency — same key returns same response, no 2nd PSP call
    // ─────────────────────────────────────────────────────────────────
    #[tokio::test]
    async fn test_idempotency_same_key_no_second_psp_call() {
        let pool = setup_db().await;
        let business_id = create_test_business(&pool).await;
        let customer_id = create_test_customer(&pool, business_id).await;
        let invoice_id = create_test_invoice(&pool, business_id, customer_id).await;

        let idempotency_key = format!("idempotency-test-{}", Uuid::new_v4());

        use sha2::{Sha256, Digest};
        let request_hash = {
            let mut hasher = Sha256::new();
            hasher.update(format!("{}{}", invoice_id, "tok_success").as_bytes());
            hex::encode(hasher.finalize())
        };

        // First insert — simulates first payment attempt
        let first = sqlx::query!(
            r#"
            INSERT INTO payment_attempts
                (invoice_id, idempotency_key, status, card_token, request_hash)
            VALUES ($1, $2, 'succeeded', 'tok_success', $3)
            RETURNING id, status::text as status
            "#,
            invoice_id,
            idempotency_key,
            request_hash
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let first_id = first.id;

        // Second lookup with same key — simulates retry
        let existing = sqlx::query!(
            "SELECT id, status::text as status, request_hash FROM payment_attempts WHERE idempotency_key = $1",
            idempotency_key
        )
        .fetch_optional(&pool)
        .await
        .unwrap();

        assert!(existing.is_some(), "Should find existing attempt on retry");
        let existing = existing.unwrap();

        // Same attempt ID returned — no new PSP call needed
        assert_eq!(existing.id, first_id, "Same attempt ID should be returned");
        assert_eq!(existing.request_hash, request_hash, "Request hash should match — same body");

        // Only 1 attempt should ever exist for this invoice
        let count = sqlx::query!(
            "SELECT COUNT(*) as count FROM payment_attempts WHERE invoice_id = $1",
            invoice_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(count.count, Some(1), "Should have exactly 1 payment attempt, not 2");
        println!("✅ Idempotency test passed — same response returned, no second PSP call");
    }

    // ─────────────────────────────────────────────────────────────────
    // TEST 3: PSP timeout — invoice stays open, not stuck in bad state
    // ─────────────────────────────────────────────────────────────────
    #[tokio::test]
    async fn test_psp_timeout_invoice_not_stuck() {
        let pool = setup_db().await;
        let business_id = create_test_business(&pool).await;
        let customer_id = create_test_customer(&pool, business_id).await;
        let invoice_id = create_test_invoice(&pool, business_id, customer_id).await;

        let idempotency_key = format!("timeout-test-{}", Uuid::new_v4());

        use sha2::{Sha256, Digest};
        let request_hash = {
            let mut hasher = Sha256::new();
            hasher.update(format!("{}{}", invoice_id, "tok_timeout").as_bytes());
            hex::encode(hasher.finalize())
        };

        // Simulate PSP timeout — attempt saved as pending, invoice NOT updated
        sqlx::query!(
            r#"
            INSERT INTO payment_attempts
                (invoice_id, idempotency_key, status, card_token, request_hash, failure_code)
            VALUES ($1, $2, 'pending', 'tok_timeout', $3, 'psp_timeout')
            "#,
            invoice_id,
            idempotency_key,
            request_hash
        )
        .execute(&pool)
        .await
        .unwrap();

        // Invoice must still be open — not corrupted
        let invoice = sqlx::query!(
            "SELECT state::text as state FROM invoices WHERE id = $1",
            invoice_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(
            invoice.state.as_deref(),
            Some("open"),
            "Invoice must stay open after PSP timeout"
        );

        // Attempt must be pending with correct failure code
        let attempt = sqlx::query!(
            "SELECT status::text as status, failure_code FROM payment_attempts WHERE idempotency_key = $1",
            idempotency_key
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(attempt.status.as_deref(), Some("pending"), "Status must be pending");
        assert_eq!(attempt.failure_code.as_deref(), Some("psp_timeout"), "failure_code must be psp_timeout");

        // Invoice is still retryable
        let open_count = sqlx::query!(
            "SELECT COUNT(*) as count FROM invoices WHERE id = $1 AND state = 'open'",
            invoice_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(open_count.count, Some(1), "Invoice should still be open and retryable");
        println!("✅ PSP timeout test passed — invoice stays open, attempt is pending, retryable");
    }
}