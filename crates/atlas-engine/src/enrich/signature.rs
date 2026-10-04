use atlas_core::{ATTR_PARAMS, Entity, LanguageFamily, ParamFact};

const RECEIVERS: [&str; 4] = ["self", "&self", "&mut self", "this"];
const STRING_TYPES: [&str; 3] = ["string", "str", "String"];
const NUMBER_TYPES: [&str; 13] = ["number", "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize"];
const FLOAT_TYPES: [&str; 2] = ["f32", "f64"];
const BOOL_TYPES: [&str; 2] = ["boolean", "bool"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shape {
  family: Option<LanguageFamily>,
  params: Vec<Option<String>>,
  returns: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SignatureMatch {
  Incompatible,
  SameArity,
  SameTypes,
}

impl SignatureMatch {
  pub(crate) fn weight(self) -> f64 {
    match self {
      Self::Incompatible => 0.0,
      Self::SameArity => 0.5,
      Self::SameTypes => 1.0,
    }
  }

  pub fn as_str(self) -> &'static str {
    match self {
      Self::Incompatible => "incompatible",
      Self::SameArity => "same_arity",
      Self::SameTypes => "same_types",
    }
  }
}

fn compact(text: &str) -> String {
  text.split_whitespace().collect()
}

fn family_of(entity: &Entity) -> Option<LanguageFamily> {
  entity.key.as_symbol().map(|symbol| symbol.family())
}

fn return_type(family: Option<LanguageFamily>, signature: &str) -> Option<String> {
  match family {
    Some(LanguageFamily::Rust) => Some(match signature.rsplit_once("->") {
      Some((_, rest)) => compact(rest.split(" where ").next().unwrap_or(rest)).trim_end_matches('{').to_owned(),
      None => "()".to_owned(),
    }),
    _ => signature
      .rsplit_once("):")
      .map(|(_, rest)| compact(rest).trim_end_matches('{').to_owned())
      .filter(|text| !text.is_empty()),
  }
}

impl Shape {
  pub(crate) fn of(entity: &Entity) -> Option<Self> {
    let signature = entity.signature.as_deref()?;
    let params: Vec<ParamFact> = entity
      .attrs
      .get(ATTR_PARAMS)
      .and_then(|value| serde_json::from_value(value.clone()).ok())
      .unwrap_or_default();
    let family = family_of(entity);
    Some(Self {
      family,
      params: params
        .iter()
        .filter(|param| !RECEIVERS.contains(&param.name.as_str()))
        .map(|param| param.type_text.as_deref().map(compact))
        .collect(),
      returns: return_type(family, signature),
    })
  }

  pub(crate) fn compare(&self, other: &Self) -> SignatureMatch {
    if self.params.len() != other.params.len() {
      return SignatureMatch::Incompatible;
    }
    let normalise = |text: &str| match self.family == other.family {
      true => text.to_owned(),
      false => type_family(text),
    };
    let types = self.params.iter().zip(&other.params).chain(std::iter::once((&self.returns, &other.returns)));
    let mut all_known = true;
    for (left, right) in types {
      match (left, right) {
        (Some(left), Some(right)) if normalise(left) != normalise(right) => return SignatureMatch::Incompatible,
        (Some(_), Some(_)) => {}
        _ => all_known = false,
      }
    }
    match all_known && !self.params.is_empty() {
      true => SignatureMatch::SameTypes,
      false => SignatureMatch::SameArity,
    }
  }
}

fn type_family(text: &str) -> String {
  let bare = text.trim_start_matches('&');
  let family = if STRING_TYPES.contains(&bare) {
    "string"
  } else if NUMBER_TYPES.contains(&bare) || FLOAT_TYPES.contains(&bare) {
    "number"
  } else if BOOL_TYPES.contains(&bare) {
    "bool"
  } else if matches!(bare, "()" | "void" | "undefined") {
    "unit"
  } else {
    return text.to_owned();
  };
  family.to_owned()
}

#[cfg(test)]
mod tests {
  use super::*;

  fn shape(family: LanguageFamily, params: &[Option<&str>], returns: Option<&str>) -> Shape {
    Shape {
      family: Some(family),
      params: params.iter().map(|param| param.map(str::to_owned)).collect(),
      returns: returns.map(str::to_owned),
    }
  }

  #[test]
  fn should_match_identical_parameter_and_return_types() {
    let sum_loop = shape(LanguageFamily::Ts, &[Some("number[]")], Some("number"));
    assert_eq!(sum_loop.compare(&sum_loop.clone()), SignatureMatch::SameTypes);
  }

  #[test]
  fn should_reject_a_different_arity_or_a_known_type_mismatch() {
    let one = shape(LanguageFamily::Ts, &[Some("string")], Some("number"));
    assert_eq!(
      one.compare(&shape(LanguageFamily::Ts, &[Some("string"), Some("number")], Some("number"))),
      SignatureMatch::Incompatible
    );
    assert_eq!(one.compare(&shape(LanguageFamily::Ts, &[Some("string")], Some("string"))), SignatureMatch::Incompatible);
  }

  #[test]
  fn should_accept_unknown_types_as_same_arity_only() {
    let typed = shape(LanguageFamily::Ts, &[Some("string")], Some("number"));
    assert_eq!(typed.compare(&shape(LanguageFamily::Ts, &[None], Some("number"))), SignatureMatch::SameArity);
    assert_eq!(typed.compare(&shape(LanguageFamily::Ts, &[Some("string")], None)), SignatureMatch::SameArity);
  }

  #[test]
  fn should_compare_type_families_across_languages_only() {
    let ts = shape(LanguageFamily::Ts, &[Some("string")], Some("number"));
    let rust = shape(LanguageFamily::Rust, &[Some("&str")], Some("i64"));
    assert_eq!(ts.compare(&rust), SignatureMatch::SameTypes);
    let rust_string = shape(LanguageFamily::Rust, &[Some("&str")], Some("i64"));
    let rust_owned = shape(LanguageFamily::Rust, &[Some("String")], Some("i64"));
    assert_eq!(rust_string.compare(&rust_owned), SignatureMatch::Incompatible);
  }

  #[test]
  fn should_read_return_types_from_rust_and_ts_signatures() {
    assert_eq!(return_type(Some(LanguageFamily::Rust), "pub fn to_cents(amount: &str) -> i64"), Some("i64".to_owned()));
    assert_eq!(return_type(Some(LanguageFamily::Rust), "fn log(line: &str)"), Some("()".to_owned()));
    assert_eq!(
      return_type(Some(LanguageFamily::Rust), "fn map<T>(value: T) -> Vec<T> where T: Clone"),
      Some("Vec<T>".to_owned())
    );
    assert_eq!(
      return_type(Some(LanguageFamily::Ts), "export function toCents(amount: string): number"),
      Some("number".to_owned())
    );
    assert_eq!(return_type(Some(LanguageFamily::Ts), "function untyped(a)"), None);
  }
}
