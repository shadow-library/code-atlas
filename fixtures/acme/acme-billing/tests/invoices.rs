use std::sync::Arc;

use acme_billing::acme_client::AcmeApiClient;
use acme_billing::store::{InMemoryInvoiceStore, Invoice, InvoiceStore};
use acme_billing::{router, AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

fn app_with(store: Arc<dyn InvoiceStore>) -> axum::Router {
  router(AppState { store, acme: AcmeApiClient::new("http://127.0.0.1:1".to_string(), "test-key".to_string()) })
}

#[tokio::test]
async fn unknown_invoice_returns_404() {
  let response = app_with(Arc::new(InMemoryInvoiceStore::default()))
    .oneshot(Request::builder().uri("/invoices/inv_missing").body(Body::empty()).unwrap())
    .await
    .unwrap();
  assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn stored_invoice_is_returned() {
  let store = Arc::new(InMemoryInvoiceStore::default());
  store.insert(Invoice { id: "inv_1".to_string(), order_id: "o1".to_string(), amount_cents: 1234 }).unwrap();
  let response = app_with(store).oneshot(Request::builder().uri("/invoices/inv_1").body(Body::empty()).unwrap()).await.unwrap();
  assert_eq!(response.status(), StatusCode::OK);
}
