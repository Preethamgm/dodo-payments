use axum::{
    extract::{State, Extension},
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::{AppState, models::Business, errors::AppError};

#[derive(Deserialize)]
pub struct RegisterWebhookRequest {
    pub url: String,
}

#[derive(Serialize)]
pub struct WebhookEndpointResponse {
    pub id: Uuid,
    pub url: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub async fn register_webhook(
    State(state): State<AppState>,
    Extension(business): Extension<Business>,
    Json(req): Json<RegisterWebhookRequest>,
) -> Result<Json<WebhookEndpointResponse>, AppError> {
    if req.url.trim().is_empty() {
        return Err(AppError::BadRequest("url is required".into()));
    }

    let endpoint = crate::db::create_webhook_endpoint(
        &state.pool,
        business.id,
        &req.url,
    )
    .await?;

    Ok(Json(WebhookEndpointResponse {
        id: endpoint.id,
        url: endpoint.url,
        created_at: endpoint.created_at,
    }))
}