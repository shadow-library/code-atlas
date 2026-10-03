use std::sync::Arc;

use acme_billing::acme_client::AcmeApiClient;
use acme_billing::store::{InvoiceStore, PgInvoiceStore};
use acme_billing::{router, AppState};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "postgres://localhost/billing".to_string());
  let pg = PgInvoiceStore::new(database_url);
  println!("billing store backed by {}", pg.database_url());
  let store: Arc<dyn InvoiceStore> = Arc::new(pg);
  let state = AppState { store, acme: AcmeApiClient::from_env()? };
  let listener = tokio::net::TcpListener::bind("0.0.0.0:4000").await?;
  axum::serve(listener, router(state)).await?;
  Ok(())
}
