use sqlx::PgPool;
use uuid::Uuid;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub async fn send_webhook(
    pool: &PgPool,
    business_id: Uuid,
    event_type: &str,
    payload: serde_json::Value,
) {
    let endpoints = crate::db::get_webhook_endpoints(pool, business_id).await;

    for endpoint in endpoints {
        let _ = crate::db::create_webhook_delivery(
            pool,
            endpoint.id,
            event_type,
            payload.clone(),
        )
        .await;
    }

    deliver_pending(pool).await;
}

pub async fn deliver_pending(pool: &PgPool) {
    let secret = std::env::var("WEBHOOK_SECRET").unwrap_or_default();
    let client = reqwest::Client::new();

    let rows = sqlx::query!(
        r#"
        SELECT wd.id, wd.endpoint_id, wd.event_type, wd.payload, wd.attempts,
               we.url
        FROM webhook_deliveries wd
        JOIN webhook_endpoints we ON we.id = wd.endpoint_id
        WHERE wd.status = 'pending'::webhook_status
          AND (wd.next_retry_at IS NULL OR wd.next_retry_at <= NOW())
          AND wd.attempts < 5
        LIMIT 10
        "#
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    for row in rows {
        let payload_str = row.payload.to_string();
        let signature = sign_payload(&secret, &payload_str);

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client
                .post(&row.url)
                .header("X-Dodo-Signature", &signature)
                .header("X-Dodo-Event", &row.event_type)
                .json(&row.payload)
                .send(),
        )
        .await;

        let success = matches!(result, Ok(Ok(r)) if r.status().is_success());

        let next_attempt = row.attempts + 1;
        let backoff_secs: i64 = match next_attempt {
            1 => 10,
            2 => 30,
            3 => 60,
            4 => 300,
            _ => 600,
        };

        if success {
            sqlx::query!(
                r#"
                UPDATE webhook_deliveries
                SET status = 'delivered'::webhook_status,
                    delivered_at = NOW(),
                    attempts = $2
                WHERE id = $1
                "#,
                row.id,
                next_attempt
            )
            .execute(pool)
            .await
            .ok();
        } else {
let next_status = if next_attempt >= 5 { "failed" } else { "pending" };
            let update_sql = format!(
                "UPDATE webhook_deliveries SET attempts = $1, status = '{}'::webhook_status, next_retry_at = NOW() + '{} seconds'::interval WHERE id = $2",
                next_status, backoff_secs
            );
            sqlx::query(&update_sql)
                .bind(next_attempt)
                .bind(row.id)
                .execute(pool)
                .await
                .ok();
        }
    }
}

fn sign_payload(secret: &str, payload: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(payload.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}