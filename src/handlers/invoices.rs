use axum::{
    extract::{Path, Query, State, Extension},
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use chrono::NaiveDate;
use crate::{AppState, models::{Business, Invoice, InvoiceState, LineItem}, errors::AppError};

#[derive(Deserialize)]
pub struct LineItemRequest {
    pub description: String,
    pub quantity: i32,
    pub unit_amount_cents: i64,
}

#[derive(Deserialize)]
pub struct CreateInvoiceRequest {
    pub customer_id: Uuid,
    pub due_date: NaiveDate,
    pub line_items: Vec<LineItemRequest>,
}

#[derive(Serialize)]
pub struct InvoiceResponse {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub state: String,
    pub total_cents: i64,
    pub due_date: NaiveDate,
    pub line_items: Vec<LineItemResponse>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
pub struct LineItemResponse {
    pub description: String,
    pub quantity: i32,
    pub unit_amount_cents: i64,
    pub total_cents: i64,
}

impl From<LineItem> for LineItemResponse {
    fn from(l: LineItem) -> Self {
        Self {
            description: l.description,
            quantity: l.quantity,
            unit_amount_cents: l.unit_amount_cents,
            total_cents: l.total_cents,
        }
    }
}

#[derive(Deserialize)]
pub struct ListInvoicesQuery {
    pub state: Option<String>,
}

pub async fn create_invoice(
    State(state): State<AppState>,
    Extension(business): Extension<Business>,
    Json(req): Json<CreateInvoiceRequest>,
) -> Result<Json<InvoiceResponse>, AppError> {
    if req.line_items.is_empty() {
        return Err(AppError::BadRequest("At least one line item is required".into()));
    }

    for item in &req.line_items {
        if item.quantity <= 0 {
            return Err(AppError::BadRequest("Quantity must be positive".into()));
        }
        if item.unit_amount_cents <= 0 {
            return Err(AppError::BadRequest("unit_amount_cents must be positive".into()));
        }
    }

    // Verify customer belongs to this business
    crate::db::get_customer(&state.pool, business.id, req.customer_id)
        .await
        .ok_or_else(|| AppError::NotFound("Customer not found".into()))?;

    let items: Vec<(String, i32, i64)> = req.line_items
        .iter()
        .map(|i| (i.description.clone(), i.quantity, i.unit_amount_cents))
        .collect();

    let invoice = crate::db::create_invoice(
        &state.pool,
        business.id,
        req.customer_id,
        req.due_date,
        &items,
    )
    .await?;

    let line_items = crate::db::get_line_items(&state.pool, invoice.id).await;

    // Queue webhook
    let pool = state.pool.clone();
    let invoice_id = invoice.id;
    let business_id = business.id;
    tokio::spawn(async move {
        crate::webhooks::send_webhook(
            &pool,
            business_id,
            "invoice.created",
            serde_json::json!({ "invoice_id": invoice_id }),
        )
        .await;
    });

    Ok(Json(to_response(invoice, line_items)))
}

pub async fn get_invoice(
    State(state): State<AppState>,
    Extension(business): Extension<Business>,
    Path(invoice_id): Path<Uuid>,
) -> Result<Json<InvoiceResponse>, AppError> {
    let invoice = crate::db::get_invoice(&state.pool, business.id, invoice_id)
        .await
        .ok_or_else(|| AppError::NotFound("Invoice not found".into()))?;

    let line_items = crate::db::get_line_items(&state.pool, invoice.id).await;
    Ok(Json(to_response(invoice, line_items)))
}

pub async fn list_invoices(
    State(state): State<AppState>,
    Extension(business): Extension<Business>,
    Query(query): Query<ListInvoicesQuery>,
) -> Result<Json<Vec<InvoiceResponse>>, AppError> {
    let state_filter = match query.state.as_deref() {
        Some("draft") => Some(InvoiceState::Draft),
        Some("open") => Some(InvoiceState::Open),
        Some("paid") => Some(InvoiceState::Paid),
        Some("void") => Some(InvoiceState::Void),
        Some("uncollectible") => Some(InvoiceState::Uncollectible),
        Some(other) => return Err(AppError::BadRequest(format!("Invalid state: {}", other))),
        None => None,
    };

    let invoices = crate::db::list_invoices(&state.pool, business.id, state_filter).await;

    let mut responses = vec![];
    for invoice in invoices {
        let line_items = crate::db::get_line_items(&state.pool, invoice.id).await;
        responses.push(to_response(invoice, line_items));
    }

    Ok(Json(responses))
}

fn to_response(invoice: Invoice, line_items: Vec<LineItem>) -> InvoiceResponse {
    InvoiceResponse {
        id: invoice.id,
        customer_id: invoice.customer_id,
        state: format!("{:?}", invoice.state).to_lowercase(),
        total_cents: invoice.total_cents,
        due_date: invoice.due_date,
        line_items: line_items.into_iter().map(Into::into).collect(),
        created_at: invoice.created_at,
    }
}