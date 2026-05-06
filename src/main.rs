use axum::{
    middleware,
    routing::{get, post},
    Router,
};
use sqlx::postgres::PgPoolOptions;


mod db;
mod errors;
mod handlers;
mod models;
mod webhooks;

#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::PgPool,
    pub psp_url: String,
    pub http_client: reqwest::Client,
    pub webhook_secret: String,
}

#[tokio::main]
async fn main() {
    // Load .env
    dotenv::dotenv().ok();

    // Tracing
    tracing_subscriber::fmt::init();

    // Database
    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set");

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await
        .expect("Failed to connect to database");

    // Run migrations
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("Failed to run migrations");

    tracing::info!("Database migrations complete");

    let psp_url = std::env::var("PSP_URL")
        .unwrap_or_else(|_| "http://localhost:3001".to_string());

    let webhook_secret = std::env::var("WEBHOOK_SECRET")
        .unwrap_or_else(|_| "supersecretkey123".to_string());

    let state = AppState {
        pool,
        psp_url,
        http_client: reqwest::Client::new(),
        webhook_secret,
    };

    // Seed a default business + API key for testing
    seed_test_data(&state).await;

    // Routes
    let protected = Router::new()
        // Customers
        .route("/customers", post(handlers::customers::create_customer))
        .route("/customers", get(handlers::customers::list_customers))
        .route("/customers/:id", get(handlers::customers::get_customer))
        // Invoices
        .route("/invoices", post(handlers::invoices::create_invoice))
        .route("/invoices", get(handlers::invoices::list_invoices))
        .route("/invoices/:id", get(handlers::invoices::get_invoice))
        // Payments
        .route("/invoices/:id/pay", post(handlers::payments::pay_invoice))
        // Webhooks
        .route("/webhooks", post(handlers::webhooks::register_webhook))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            handlers::auth::auth_middleware,
        ));

    let app = Router::new()
        .merge(protected)
        .with_state(state);

    let addr = "0.0.0.0:3000";
    tracing::info!("Server running on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn seed_test_data(state: &AppState) {
    // Create a test business if none exists
    let existing = sqlx::query!("SELECT id FROM businesses LIMIT 1")
        .fetch_optional(&state.pool)
        .await
        .unwrap();

    if existing.is_some() {
        tracing::info!("Test data already seeded");
        return;
    }

    // Insert business
    let business = sqlx::query!(
        "INSERT INTO businesses (name, email) VALUES ($1, $2) RETURNING id",
        "Test Business",
        "test@business.com"
    )
    .fetch_one(&state.pool)
    .await
    .unwrap();

    // Generate API key
    let raw_key = handlers::auth::generate_api_key();
    let key_hash = handlers::auth::hash_key(&raw_key);
    let key_prefix = &raw_key[..7];

    sqlx::query!(
        "INSERT INTO api_keys (business_id, key_hash, key_prefix) VALUES ($1, $2, $3)",
        business.id,
        key_hash,
        key_prefix
    )
    .execute(&state.pool)
    .await
    .unwrap();

    tracing::info!("═══════════════════════════════════════");
    tracing::info!("  TEST API KEY: {}", raw_key);
    tracing::info!("  Save this! It won't be shown again.");
    tracing::info!("═══════════════════════════════════════");
}
#[cfg(test)]
mod tests;