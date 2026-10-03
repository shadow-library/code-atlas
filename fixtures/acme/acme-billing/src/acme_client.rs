use serde::Deserialize;

use crate::error::ApiError;

#[derive(Debug, Deserialize)]
pub struct AcmeOrder {
  pub id: String,
  #[serde(rename = "totalCents")]
  pub total_cents: i64,
}

#[derive(Clone)]
pub struct AcmeApiClient {
  http: reqwest::Client,
  base_url: String,
  api_key: String,
}

impl AcmeApiClient {
  pub fn new(base_url: String, api_key: String) -> Self {
    Self { http: reqwest::Client::new(), base_url, api_key }
  }

  pub fn from_env() -> Result<Self, std::env::VarError> {
    let base_url = std::env::var("ACME_API_URL")?;
    let api_key = std::env::var("BILLING_API_KEY")?;
    Ok(Self::new(base_url, api_key))
  }

  pub async fn get_order(&self, order_id: &str) -> Result<AcmeOrder, ApiError> {
    let url = format!("{}/orders/{}", self.base_url, order_id);
    let response = self.http.get(url).header("x-api-key", &self.api_key).send().await?.error_for_status()?;
    Ok(response.json::<AcmeOrder>().await?)
  }
}
