use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use atlas_core::{CertaintyClass, Owner};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::{StoreError, StoreResult};

pub(crate) fn to_json<T: Serialize>(value: &T) -> StoreResult<String> {
  Ok(serde_json::to_string(value)?)
}

pub(crate) fn from_json<T: DeserializeOwned>(what: &'static str, text: &str) -> StoreResult<T> {
  serde_json::from_str(text).map_err(|error| StoreError::corrupt(what, error.to_string()))
}

pub(crate) fn to_sql_int(what: &'static str, value: u64) -> StoreResult<i64> {
  i64::try_from(value).map_err(|_| StoreError::OutOfRange { what, value })
}

pub(crate) fn from_sql_int(what: &'static str, value: i64) -> StoreResult<u64> {
  u64::try_from(value).map_err(|_| StoreError::corrupt(what, format!("negative value {value}")))
}

pub(crate) fn now_unix_ms() -> i64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
}

pub(crate) fn certainty_tier(class: CertaintyClass) -> i64 {
  match class {
    CertaintyClass::CompilerVerified => 5,
    CertaintyClass::Syntactic | CertaintyClass::Declared => 4,
    CertaintyClass::Resolved => 3,
    CertaintyClass::Heuristic => 2,
    CertaintyClass::Inferred => 1,
  }
}

pub(crate) fn owner_label(owner: &Owner) -> String {
  match owner {
    Owner::File { repo, path } => format!("file {repo}:{path}"),
    Owner::Project { pass } => format!("project pass {pass}"),
  }
}

pub(crate) fn write_canonical_json(value: &Value, out: &mut String) {
  match value {
    Value::Object(map) => {
      let mut entries: Vec<_> = map.iter().collect();
      entries.sort_by(|left, right| left.0.cmp(right.0));
      out.push('{');
      for (index, (key, item)) in entries.into_iter().enumerate() {
        if index > 0 {
          out.push(',');
        }
        let _ = write!(out, "{}:", Value::String(key.clone()));
        write_canonical_json(item, out);
      }
      out.push('}');
    }
    Value::Array(items) => {
      out.push('[');
      for (index, item) in items.iter().enumerate() {
        if index > 0 {
          out.push(',');
        }
        write_canonical_json(item, out);
      }
      out.push(']');
    }
    scalar => {
      let _ = write!(out, "{scalar}");
    }
  }
}

pub(crate) fn canonical_json<T: Serialize>(value: &T) -> StoreResult<String> {
  let mut out = String::new();
  write_canonical_json(&serde_json::to_value(value)?, &mut out);
  Ok(out)
}

#[cfg(test)]
mod tests {
  use atlas_core::{Certainty, Confidence};

  use super::*;

  #[test]
  fn should_render_objects_with_sorted_keys_at_every_depth() {
    let value = serde_json::json!({ "b": [{ "z": 1, "a": "x\"y" }], "a": null });
    let mut out = String::new();
    write_canonical_json(&value, &mut out);
    assert_eq!(out, r#"{"a":null,"b":[{"a":"x\"y","z":1}]}"#);
  }

  #[test]
  fn should_order_tiers_consistently_with_core_certainty_ranks() {
    let lowest = Confidence::new(0.0).unwrap();
    let representative = |class: CertaintyClass, confidence: Confidence| match class {
      CertaintyClass::CompilerVerified => Certainty::CompilerVerified,
      CertaintyClass::Syntactic => Certainty::Syntactic,
      CertaintyClass::Resolved => Certainty::Resolved,
      CertaintyClass::Declared => Certainty::Declared,
      CertaintyClass::Heuristic => Certainty::heuristic(confidence, "r"),
      CertaintyClass::Inferred => Certainty::Inferred {
        confidence,
        provider: "p".into(),
        model: "m".into(),
        rationale: "r".into(),
      },
    };
    for left in CertaintyClass::ALL {
      for right in CertaintyClass::ALL {
        if certainty_tier(left) < certainty_tier(right) {
          assert!(
            representative(left, Confidence::CERTAIN).rank() < representative(right, lowest).rank(),
            "{left} must rank below {right}"
          );
        }
      }
    }
  }
}
