use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use sha2::{Sha256, Digest};
use crate::{AppState, errors::AppError};

pub async fn auth_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let api_key = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(AppError::Unauthorized)?;

    let key_hash = hash_key(api_key);

    let business = crate::db::get_business_by_api_key(&state.pool, &key_hash)
        .await
        .ok_or(AppError::Unauthorized)?;

    req.extensions_mut().insert(business);
    Ok(next.run(req).await)
}

pub fn hash_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn generate_api_key() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let key: String = (0..32)
        .map(|_| rng.sample(rand::distributions::Alphanumeric) as char)
        .collect();
    format!("sk_{}", key)
}