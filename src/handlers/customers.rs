use axum::{
    extract::{Path, State, Extension},
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::{AppState, models::{Business, Customer}, errors::AppError};

#[derive(Deserialize)]
pub struct CreateCustomerRequest {
    pub name: String,
    pub email: String,
}

#[derive(Serialize)]
pub struct CustomerResponse {
    pub id: Uuid,
    pub name: String,
    pub email: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<Customer> for CustomerResponse {
    fn from(c: Customer) -> Self {
        Self {
            id: c.id,
            name: c.name,
            email: c.email,
            created_at: c.created_at,
        }
    }
}

pub async fn create_customer(
    State(state): State<AppState>,
    Extension(business): Extension<Business>,
    Json(req): Json<CreateCustomerRequest>,
) -> Result<Json<CustomerResponse>, AppError> {
    if req.name.trim().is_empty() || req.email.trim().is_empty() {
        return Err(AppError::BadRequest("name and email are required".into()));
    }

    let customer = crate::db::create_customer(
        &state.pool,
        business.id,
        &req.name,
        &req.email,
    )
    .await
    .map_err(|e| {
        if e.to_string().contains("unique") {
            AppError::Conflict("Customer with this email already exists".into())
        } else {
            AppError::Internal(e.to_string())
        }
    })?;

    Ok(Json(customer.into()))
}

pub async fn get_customer(
    State(state): State<AppState>,
    Extension(business): Extension<Business>,
    Path(customer_id): Path<Uuid>,
) -> Result<Json<CustomerResponse>, AppError> {
    let customer = crate::db::get_customer(&state.pool, business.id, customer_id)
        .await
        .ok_or_else(|| AppError::NotFound("Customer not found".into()))?;

    Ok(Json(customer.into()))
}

pub async fn list_customers(
    State(state): State<AppState>,
    Extension(business): Extension<Business>,
) -> Result<Json<Vec<CustomerResponse>>, AppError> {
    let customers = crate::db::list_customers(&state.pool, business.id).await;
    Ok(Json(customers.into_iter().map(Into::into).collect()))
}