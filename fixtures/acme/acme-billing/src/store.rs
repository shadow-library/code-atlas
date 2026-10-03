use std::collections::HashMap;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::error::ApiError;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Invoice {
  pub id: String,
  pub order_id: String,
  pub amount_cents: i64,
}

pub trait InvoiceStore: Send + Sync {
  fn insert(&self, invoice: Invoice) -> Result<Invoice, ApiError>;
  fn find(&self, id: &str) -> Option<Invoice>;
}

pub struct PgInvoiceStore {
  database_url: String,
  rows: RwLock<HashMap<String, Invoice>>,
}

impl PgInvoiceStore {
  pub fn new(database_url: String) -> Self {
    Self { database_url, rows: RwLock::new(HashMap::new()) }
  }

  pub fn database_url(&self) -> &str {
    &self.database_url
  }
}

impl InvoiceStore for PgInvoiceStore {
  fn insert(&self, invoice: Invoice) -> Result<Invoice, ApiError> {
    let mut rows = self.rows.write().map_err(|error| ApiError::Store(error.to_string()))?;
    rows.insert(invoice.id.clone(), invoice.clone());
    Ok(invoice)
  }

  fn find(&self, id: &str) -> Option<Invoice> {
    self.rows.read().ok()?.get(id).cloned()
  }
}

#[derive(Default)]
pub struct InMemoryInvoiceStore {
  rows: RwLock<Vec<Invoice>>,
}

impl InvoiceStore for InMemoryInvoiceStore {
  fn insert(&self, invoice: Invoice) -> Result<Invoice, ApiError> {
    let mut rows = self.rows.write().map_err(|error| ApiError::Store(error.to_string()))?;
    rows.push(invoice.clone());
    Ok(invoice)
  }

  fn find(&self, id: &str) -> Option<Invoice> {
    self.rows.read().ok()?.iter().find(|invoice| invoice.id == id).cloned()
  }
}
