pub async fn load_order(http: &reqwest::Client, base_url: &str, api_key: &str) -> reqwest::Result<reqwest::Response> {
  let url = format!("{}/orders/{}", base_url, "o-1");
  http.get(url).header("x-api-key", api_key).send().await
}

#[cfg(test)]
mod fixtures {
  pub async fn load_order_in_test(http: &reqwest::Client, base_url: &str, api_key: &str) -> reqwest::Result<reqwest::Response> {
    let url = format!("{}/orders/{}", base_url, "o-2");
    http.get(url).header("x-api-key", api_key).send().await
  }
}
