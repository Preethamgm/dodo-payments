use axum::{routing::post, Router, Json, extract::State};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::sleep;

#[derive(Deserialize)]
struct PspRequest {
    card_token: String,
    amount_cents: i64,
}

#[derive(Serialize)]
struct PspResponse {
    status: String,
    psp_ref: Option<String>,
    code: Option<String>,
}

async fn charge(Json(req): Json<PspRequest>) -> Json<PspResponse> {
    match req.card_token.as_str() {
        "tok_success" => {
            sleep(Duration::from_millis(100)).await;
            Json(PspResponse {
                status: "succeeded".to_string(),
                psp_ref: Some(uuid::Uuid::new_v4().to_string()),
                code: None,
            })
        }
        "tok_insufficient_funds" => {
            sleep(Duration::from_millis(100)).await;
            Json(PspResponse {
                status: "failed".to_string(),
                psp_ref: None,
                code: Some("insufficient_funds".to_string()),
            })
        }
        "tok_card_declined" => {
            sleep(Duration::from_millis(100)).await;
            Json(PspResponse {
                status: "failed".to_string(),
                psp_ref: None,
                code: Some("card_declined".to_string()),
            })
        }
        "tok_timeout" => {
            sleep(Duration::from_secs(30)).await;
            Json(PspResponse {
                status: "succeeded".to_string(),
                psp_ref: Some(uuid::Uuid::new_v4().to_string()),
                code: None,
            })
        }
        "tok_network_error" => {
            panic!("Simulated network error");
        }
        _ => {
            Json(PspResponse {
                status: "failed".to_string(),
                psp_ref: None,
                code: Some("unknown_token".to_string()),
            })
        }
    }
}

#[tokio::main]
async fn main() {
    let port = std::env::var("PSP_PORT").unwrap_or_else(|_| "3001".to_string());
    let addr = format!("0.0.0.0:{}", port);

    let app = Router::new().route("/charge", post(charge));

    println!("Mock PSP running on {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}