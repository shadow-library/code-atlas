use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
pub const SERVER_NOT_INITIALIZED: i64 = -32002;
pub const REQUEST_FAILED: i64 = -32803;
pub const SERVER_CANCELLED: i64 = -32802;
pub const CONTENT_MODIFIED: i64 = -32801;
pub const REQUEST_CANCELLED: i64 = -32800;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
  Number(i64),
  String(String),
}

impl From<i64> for RequestId {
  fn from(id: i64) -> Self {
    Self::Number(id)
  }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseError {
  pub code: i64,
  #[serde(default)]
  pub message: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub data: Option<Value>,
}

impl ResponseError {
  pub fn new(code: i64, message: impl Into<String>) -> Self {
    Self {
      code,
      message: message.into(),
      data: None,
    }
  }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
  Request { id: RequestId, method: String, params: Value },
  Notification { method: String, params: Value },
  Response { id: Option<RequestId>, outcome: Result<Value, ResponseError> },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum MessageError {
  #[error("message is not valid JSON: {0}")]
  InvalidJson(String),
  #[error("message is not a JSON object")]
  NotAnObject,
  #[error("message has an invalid id {0}")]
  InvalidId(String),
  #[error("message method is not a string")]
  InvalidMethod,
  #[error("message has neither a method nor an id")]
  Unclassifiable,
  #[error("error response is malformed: {0}")]
  InvalidError(String),
}

impl Message {
  pub fn parse(body: &[u8]) -> Result<Self, MessageError> {
    let value: Value = serde_json::from_slice(body).map_err(|error| MessageError::InvalidJson(error.to_string()))?;
    let Value::Object(mut object) = value else {
      return Err(MessageError::NotAnObject);
    };
    let id = match object.remove("id") {
      None | Some(Value::Null) => None,
      Some(raw) => Some(serde_json::from_value::<RequestId>(raw.clone()).map_err(|_| MessageError::InvalidId(raw.to_string()))?),
    };
    let params = object.remove("params").unwrap_or(Value::Null);
    match (object.remove("method"), id) {
      (Some(Value::String(method)), Some(id)) => Ok(Self::Request { id, method, params }),
      (Some(Value::String(method)), None) => Ok(Self::Notification { method, params }),
      (Some(_), _) => Err(MessageError::InvalidMethod),
      (None, id) => Self::response(id, object),
    }
  }

  fn response(id: Option<RequestId>, mut object: Map<String, Value>) -> Result<Self, MessageError> {
    match (object.remove("error"), object.remove("result")) {
      (Some(error), _) if !error.is_null() => {
        let error = serde_json::from_value::<ResponseError>(error).map_err(|error| MessageError::InvalidError(error.to_string()))?;
        Ok(Self::Response { id, outcome: Err(error) })
      }
      (_, result) if id.is_some() => Ok(Self::Response {
        id,
        outcome: Ok(result.unwrap_or(Value::Null)),
      }),
      _ => Err(MessageError::Unclassifiable),
    }
  }

  pub fn to_bytes(&self) -> Vec<u8> {
    let mut object = Map::new();
    object.insert("jsonrpc".to_owned(), Value::from("2.0"));
    match self {
      Self::Request { id, method, params } => {
        object.insert("id".to_owned(), id_value(id));
        object.insert("method".to_owned(), Value::from(method.as_str()));
        insert_params(&mut object, params);
      }
      Self::Notification { method, params } => {
        object.insert("method".to_owned(), Value::from(method.as_str()));
        insert_params(&mut object, params);
      }
      Self::Response { id, outcome } => {
        object.insert("id".to_owned(), id.as_ref().map_or(Value::Null, id_value));
        match outcome {
          Ok(result) => object.insert("result".to_owned(), result.clone()),
          Err(error) => object.insert("error".to_owned(), serde_json::to_value(error).unwrap_or(Value::Null)),
        };
      }
    }
    Value::Object(object).to_string().into_bytes()
  }
}

fn id_value(id: &RequestId) -> Value {
  match id {
    RequestId::Number(number) => Value::from(*number),
    RequestId::String(text) => Value::from(text.as_str()),
  }
}

fn insert_params(object: &mut Map<String, Value>, params: &Value) {
  if !params.is_null() {
    object.insert("params".to_owned(), params.clone());
  }
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;

  fn parse(value: Value) -> Result<Message, MessageError> {
    Message::parse(value.to_string().as_bytes())
  }

  #[test]
  fn should_classify_requests_notifications_and_responses() {
    assert_eq!(
      parse(json!({"jsonrpc": "2.0", "id": "a", "method": "m", "params": [1]})),
      Ok(Message::Request {
        id: RequestId::String("a".to_owned()),
        method: "m".to_owned(),
        params: json!([1])
      })
    );
    assert_eq!(
      parse(json!({"jsonrpc": "2.0", "method": "n"})),
      Ok(Message::Notification {
        method: "n".to_owned(),
        params: Value::Null
      })
    );
    assert_eq!(
      parse(json!({"jsonrpc": "2.0", "id": 4, "result": null})),
      Ok(Message::Response {
        id: Some(RequestId::Number(4)),
        outcome: Ok(Value::Null)
      })
    );
    assert_eq!(
      parse(json!({"jsonrpc": "2.0", "id": 4})),
      Ok(Message::Response {
        id: Some(RequestId::Number(4)),
        outcome: Ok(Value::Null)
      })
    );
    assert_eq!(
      parse(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "bad"}})),
      Ok(Message::Response {
        id: None,
        outcome: Err(ResponseError::new(PARSE_ERROR, "bad"))
      })
    );
  }

  #[test]
  fn should_reject_malformed_envelopes() {
    assert!(matches!(Message::parse(b"{not json"), Err(MessageError::InvalidJson(_))));
    assert!(matches!(Message::parse(b"{\"a\":\"\xff\"}"), Err(MessageError::InvalidJson(_))));
    assert_eq!(parse(json!([1])), Err(MessageError::NotAnObject));
    assert_eq!(parse(json!({"id": 1.5, "result": 1})), Err(MessageError::InvalidId("1.5".to_owned())));
    assert_eq!(parse(json!({"id": 1, "method": 3})), Err(MessageError::InvalidMethod));
    assert_eq!(parse(json!({"jsonrpc": "2.0"})), Err(MessageError::Unclassifiable));
    assert!(matches!(parse(json!({"id": 1, "error": "boom"})), Err(MessageError::InvalidError(_))));
  }

  #[test]
  fn should_serialize_without_null_params_and_with_null_results() {
    let request = Message::Request {
      id: RequestId::Number(1),
      method: "shutdown".to_owned(),
      params: Value::Null,
    };
    let response = Message::Response {
      id: Some(RequestId::String("x".to_owned())),
      outcome: Ok(Value::Null),
    };

    let decode = |bytes: Vec<u8>| serde_json::from_slice::<Value>(&bytes).unwrap();

    assert_eq!(decode(request.to_bytes()), json!({"id": 1, "jsonrpc": "2.0", "method": "shutdown"}));
    assert_eq!(decode(response.to_bytes()), json!({"id": "x", "jsonrpc": "2.0", "result": null}));
  }
}
