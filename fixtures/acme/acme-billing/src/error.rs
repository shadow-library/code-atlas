use std::fmt;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug)]
pub enum ApiError {
  NotFound(String),
  Upstream(String),
  Store(String),
}

impl fmt::Display for ApiError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      ApiError::NotFound(id) => write!(f, "invoice {id} not found"),
      ApiError::Upstream(message) => write!(f, "upstream error: {message}"),
      ApiError::Store(message) => write!(f, "store error: {message}"),
    }
  }
}

impl std::error::Error for ApiError {}

impl From<reqwest::Error> for ApiError {
  fn from(error: reqwest::Error) -> Self {
    ApiError::Upstream(error.to_string())
  }
}

impl IntoResponse for ApiError {
  fn into_response(self) -> Response {
    let status = match self {
      ApiError::NotFound(_) => StatusCode::NOT_FOUND,
      ApiError::Upstream(_) => StatusCode::BAD_GATEWAY,
      ApiError::Store(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, self.to_string()).into_response()
  }
}
