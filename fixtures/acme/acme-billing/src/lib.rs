pub mod acme_client;
pub mod error;
pub mod events;
pub mod handlers;
pub mod money;
pub mod store;

use std::sync::Arc;

use axum::{
  routing::{get, post},
  Router,
};

use crate::acme_client::AcmeApiClient;
use crate::handlers::{create_invoice, get_invoice};
use crate::store::InvoiceStore;

#[derive(Clone)]
pub struct AppState {
  pub store: Arc<dyn InvoiceStore>,
  pub acme: AcmeApiClient,
}

pub fn router(state: AppState) -> Router {
  Router::new().route("/invoices", post(create_invoice)).route("/invoices/{id}", get(get_invoice)).with_state(state)
}
