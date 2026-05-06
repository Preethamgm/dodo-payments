use axum::{
    extract::{Path, State, Extension},
    Json,
};
use axum_extra::TypedHeader;
use headers::{Header, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use sha2::{Sha256, Digest};
use crate::{
    AppState,
    models::{Business, InvoiceState, PaymentStatus},
    errors::AppError,
};

#[derive(Deserialize)]
pub struct PayRequest {
    pub card_token: String,
}

#[derive(Serialize)]
pub struct PayResponse {
    pub payment_attempt_id: Uuid,
    pub status: String,
    pub psp_ref: Option<String>,
    pub failure_code: Option<String>,
}

#[derive(Deserialize)]
struct PspResponse {
    status: String,
    psp_ref: Option<String>,
    code: Option<String>,
}

// Custom header for Idempotency-Key
pub struct IdempotencyKey(pub String);

static IDEMPOTENCY_KEY_NAME: std::sync::OnceLock<HeaderName> = std::sync::OnceLock::new();

impl Header for IdempotencyKey {
    fn name() -> &'static HeaderName {
        IDEMPOTENCY_KEY_NAME.get_or_init(|| {
            HeaderName::from_static("idempotency-key")
        })
    }

    fn decode<'i, I>(values: &mut I) -> Result<Self, headers::Error>
    where
        I: Iterator<Item = &'i HeaderValue>,
    {
        let value = values.next().ok_or_else(headers::Error::invalid)?;
        Ok(IdempotencyKey(
            value.to_str().map_err(|_| headers::Error::invalid())?.to_string(),
        ))
    }

    fn encode<E: Extend<HeaderValue>>(&self, values: &mut E) {
        values.extend(std::iter::once(
            HeaderValue::from_str(&self.0).unwrap(),
        ));
    }
}

pub async fn pay_invoice(
    State(state): State<AppState>,
    Extension(business): Extension<Business>,
    Path(invoice_id): Path<Uuid>,
    TypedHeader(idempotency_key): TypedHeader<IdempotencyKey>,
    Json(req): Json<PayRequest>,
) -> Result<Json<PayResponse>, AppError> {
    // 1. Compute request hash
    let request_hash = {
        let mut hasher = Sha256::new();
        hasher.update(format!("{}{}", invoice_id, req.card_token).as_bytes());
        hex::encode(hasher.finalize())
    };

    // 2. Check idempotency key
    if let Some(existing) = crate::db::get_payment_attempt_by_idempotency_key(
        &state.pool,
        &idempotency_key.0,
    )
    .await
    {
        if existing.request_hash != request_hash {
            return Err(AppError::Conflict(
                "Idempotency key reused with different request body".into(),
            ));
        }
        return Ok(Json(PayResponse {
            payment_attempt_id: existing.id,
            status: format!("{:?}", existing.status).to_lowercase(),
            psp_ref: existing.psp_ref,
            failure_code: existing.failure_code,
        }));
    }

    // 3. Lock invoice row and check state
    let invoice = sqlx::query!(
        r#"
        SELECT id, state AS "state: InvoiceState", business_id
        FROM invoices
        WHERE id = $1 AND business_id = $2
        FOR UPDATE
        "#,
        invoice_id,
        business.id
    )
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
    .ok_or_else(|| AppError::NotFound("Invoice not found".into()))?;

    // 4. Validate state transition
    match invoice.state {
        InvoiceState::Paid => return Err(AppError::InvalidStateTransition("Invoice is already paid".into())),
        InvoiceState::Void => return Err(AppError::InvalidStateTransition("Cannot pay a void invoice".into())),
        InvoiceState::Uncollectible => return Err(AppError::InvalidStateTransition("Cannot pay an uncollectible invoice".into())),
        InvoiceState::Draft => return Err(AppError::InvalidStateTransition("Cannot pay a draft invoice".into())),
        InvoiceState::Open => {}
    }

    // 5. Create pending payment attempt
    let attempt = crate::db::create_payment_attempt(
        &state.pool,
        invoice_id,
        &idempotency_key.0,
        &req.card_token,
        &request_hash,
    )
    .await?;

    // 6. Call PSP with timeout
    let psp_url = format!("{}/charge", state.psp_url);
    let psp_result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        state.http_client
            .post(&psp_url)
            .json(&serde_json::json!({
                "card_token": req.card_token,
                "amount_cents": 0i64,
            }))
            .send(),
    )
    .await;

    // 7. Handle result
    let (status, psp_ref, failure_code, new_invoice_state) = match psp_result {
        Err(_) => (PaymentStatus::Pending, None, Some("psp_timeout".to_string()), None),
        Ok(Err(_)) => (PaymentStatus::Failed, None, Some("network_error".to_string()), Some(InvoiceState::Open)),
        Ok(Ok(resp)) => {
            match resp.json::<PspResponse>().await {
                Ok(psp) if psp.status == "succeeded" => (
                    PaymentStatus::Succeeded, psp.psp_ref, None, Some(InvoiceState::Paid),
                ),
                Ok(psp) => (
                    PaymentStatus::Failed, None, psp.code, Some(InvoiceState::Open),
                ),
                Err(_) => (
                    PaymentStatus::Failed, None, Some("invalid_psp_response".to_string()), Some(InvoiceState::Open),
                ),
            }
        }
    };

    // 8. Persist outcome
    crate::db::update_payment_attempt(
        &state.pool,
        attempt.id,
        status.clone(),
        psp_ref.clone(),
        failure_code.clone(),
    )
    .await?;

    if let Some(new_state) = new_invoice_state {
        crate::db::update_invoice_state(&state.pool, invoice_id, new_state.clone()).await?;

        let pool = state.pool.clone();
        let business_id = business.id;
        let event = match new_state {
            InvoiceState::Paid => "invoice.paid",
            _ => "invoice.payment_failed",
        };
        tokio::spawn(async move {
            crate::webhooks::send_webhook(
                &pool,
                business_id,
                event,
                serde_json::json!({ "invoice_id": invoice_id, "attempt_id": attempt.id }),
            )
            .await;
        });
    }

    Ok(Json(PayResponse {
        payment_attempt_id: attempt.id,
        status: format!("{:?}", status).to_lowercase(),
        psp_ref,
        failure_code,
    }))
}