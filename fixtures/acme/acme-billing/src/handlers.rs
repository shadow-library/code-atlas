use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::error::ApiError;
use crate::store::{Invoice, InvoiceStore};
use crate::AppState;

#[derive(Deserialize)]
pub struct CreateInvoice {
  pub order_id: String,
}

fn persist(store: &dyn InvoiceStore, invoice: Invoice) -> Result<Invoice, ApiError> {
  store.insert(invoice)
}

pub async fn create_invoice(State(state): State<AppState>, Json(request): Json<CreateInvoice>) -> Result<(StatusCode, Json<Invoice>), ApiError> {
  let order = state.acme.get_order(&request.order_id).await?;
  let invoice = Invoice { id: format!("inv_{}", order.id), order_id: order.id, amount_cents: order.total_cents };
  let saved = persist(state.store.as_ref(), invoice)?;
  Ok((StatusCode::CREATED, Json(saved)))
}

pub async fn get_invoice(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Invoice>, ApiError> {
  state.store.find(&id).map(Json).ok_or(ApiError::NotFound(id))
}
