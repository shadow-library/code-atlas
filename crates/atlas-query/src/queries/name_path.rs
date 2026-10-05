use atlas_core::{Entity, EntityKey, LanguageFamily};

use crate::error::{QueryError, QueryResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
  Relative,
  Absolute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameMatch {
  Exact,
  Substring,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamePath {
  pub anchor: Anchor,
  pub segments: Vec<String>,
  pub index: Option<usize>,
  pub literal: Option<String>,
}

impl NamePath {
  pub fn parse(text: &str) -> QueryResult<Self> {
    let text = text.trim();
    let (anchor, body) = match text.strip_prefix('/') {
      Some(rest) => (Anchor::Absolute, rest),
      None => (Anchor::Relative, text),
    };
    if body.trim().is_empty() {
      return Err(QueryError::invalid(format!("empty name path {text:?}")));
    }
    let body = rust_impl_path(body).unwrap_or_else(|| body.to_owned());
    let segments: Vec<&str> = match () {
      _ if body.contains('/') => body.split('/').flat_map(|segment| segment.split("::")).collect(),
      _ if body.contains("::") => body.split("::").collect(),
      _ if body.contains('.') => body.split('.').collect(),
      _ => vec![body.as_str()],
    };
    let literal = (anchor == Anchor::Relative && segments.len() > 1).then(|| body.clone());
    Self::from_segments(text, anchor, segments, literal)
  }

  pub fn literal_fallback(&self) -> Option<Self> {
    let body = self.literal.as_deref()?;
    Self::from_segments(body, self.anchor, vec![body], None).ok()
  }

  fn from_segments(text: &str, anchor: Anchor, segments: Vec<&str>, literal: Option<String>) -> QueryResult<Self> {
    let mut segments: Vec<String> = segments.into_iter().map(|segment| segment.trim().to_owned()).collect();
    if segments.iter().any(String::is_empty) {
      return Err(QueryError::invalid(format!("name path {text:?} has an empty segment")));
    }
    if segments[..segments.len() - 1].iter().any(|segment| overload_index(segment).is_some()) {
      return Err(QueryError::invalid(format!(
        "name path {text:?}: an overload index [i] is only allowed on the last segment"
      )));
    }
    let last = segments.len() - 1;
    let index = match overload_index(&segments[last]) {
      Some((name, index)) => {
        segments[last] = name.to_owned();
        Some(index)
      }
      None => None,
    };
    Ok(Self { anchor, segments, index, literal })
  }

  pub fn name(&self) -> &str {
    self.segments.last().map_or("", String::as_str)
  }

  pub fn qualifier(&self) -> &[String] {
    &self.segments[..self.segments.len().saturating_sub(1)]
  }

  pub fn matches_qualified(&self, qualified: &[String], crate_name: Option<&str>, rule: NameMatch) -> bool {
    let Some(owners) = self.owners_if_named(qualified, rule) else { return false };
    let qualifier = self.qualifier();
    if let Some((head, rest)) = qualifier.split_first()
      && (head == "crate" || crate_name == Some(head.as_str()))
      && same(&owners, rest)
    {
      return true;
    }
    owners.len() >= qualifier.len() && same(&owners[owners.len() - qualifier.len()..], qualifier)
  }

  pub fn may_match_in_file(&self, item_path: &[String]) -> bool {
    let names_trait = self.qualifier().iter().any(|segment| is_trait_segment(segment));
    item_path.iter().filter(|segment| names_trait || !is_trait_segment(segment)).count() <= self.segments.len()
  }

  pub fn matches_in_file(&self, in_file: &[String], rule: NameMatch) -> bool {
    self.owners_if_named(in_file, rule).is_some_and(|owners| same(&owners, self.qualifier()))
  }

  fn owners_if_named<'s>(&self, segments: &'s [String], rule: NameMatch) -> Option<Vec<&'s str>> {
    let names_trait = self.qualifier().iter().any(|segment| is_trait_segment(segment));
    let mut kept: Vec<&str> = segments.iter().map(String::as_str).filter(|segment| names_trait || !is_trait_segment(segment)).collect();
    let last = kept.pop()?;
    let accepted = match rule {
      NameMatch::Exact => last == self.name(),
      NameMatch::Substring => last.contains(self.name()),
    };
    accepted.then_some(kept)
  }
}

fn same(owners: &[&str], wanted: &[String]) -> bool {
  owners.len() == wanted.len() && owners.iter().zip(wanted).all(|(owner, wanted)| owner == wanted)
}

pub(crate) fn crate_name(entity: &Entity) -> Option<String> {
  let symbol = entity.key.as_symbol().filter(|symbol| symbol.family() == LanguageFamily::Rust)?;
  let package = symbol.package();
  (!package.contains('/')).then(|| package.replace('-', "_"))
}

pub(crate) fn qualified_segments(entity: &Entity) -> Vec<String> {
  match &entity.key {
    EntityKey::Symbol(symbol) => symbol.module_path().iter().chain(symbol.item_path()).cloned().collect(),
    _ => vec![entity.name.clone()],
  }
}

pub(crate) fn file_module_prefix(top: &Entity) -> &[String] {
  let EntityKey::Symbol(symbol) = &top.key else { return &[] };
  let module = symbol.module_path();
  match symbol.item_path().is_empty() {
    true => &module[..module.len().saturating_sub(1)],
    false => module,
  }
}

pub(crate) fn is_trait_segment(segment: &str) -> bool {
  segment.len() > 2 && segment.starts_with('<') && segment.ends_with('>')
}

fn overload_index(segment: &str) -> Option<(&str, usize)> {
  let inner = segment.strip_suffix(']')?;
  let open = inner.rfind('[')?;
  let (name, digits) = (&inner[..open], &inner[open + 1..]);
  if name.trim().is_empty() || digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
    return None;
  }
  Some((name.trim_end(), digits.parse().ok()?))
}

fn rust_impl_path(text: &str) -> Option<String> {
  let rest = text.strip_prefix('<')?;
  let (inner, tail) = rest.split_once(">::")?;
  let (self_type, trait_name) = inner.split_once(" as ")?;
  Some(format!("{}::<{}>::{tail}", self_type.trim(), trait_name.trim()))
}

#[cfg(test)]
mod tests {
  use super::*;

  type Shape = (Anchor, Vec<String>, Option<usize>);

  fn path(anchor: Anchor, segments: &[&str], index: Option<usize>) -> Shape {
    (anchor, owned(segments), index)
  }

  fn shape(text: &str) -> Shape {
    let parsed = NamePath::parse(text).unwrap();
    (parsed.anchor, parsed.segments, parsed.index)
  }

  fn matches(pattern: &str, qualified: &[String], in_file: &[String], rule: NameMatch) -> bool {
    let pattern = NamePath::parse(pattern).unwrap();
    match pattern.anchor {
      Anchor::Relative => pattern.matches_qualified(qualified, Some("acme_billing"), rule),
      Anchor::Absolute => pattern.matches_in_file(in_file, rule),
    }
  }

  fn owned(segments: &[&str]) -> Vec<String> {
    segments.iter().map(|segment| (*segment).to_owned()).collect()
  }

  #[test]
  fn should_parse_relative_absolute_and_indexed_name_paths() {
    assert_eq!(shape("UsersService/findOne"), path(Anchor::Relative, &["UsersService", "findOne"], None));
    assert_eq!(shape("/UsersService/findOne"), path(Anchor::Absolute, &["UsersService", "findOne"], None));
    assert_eq!(shape("parse[1]"), path(Anchor::Relative, &["parse"], Some(1)));
    assert_eq!(shape("  toCents "), path(Anchor::Relative, &["toCents"], None));
    assert_eq!(shape("/toCents[0]"), path(Anchor::Absolute, &["toCents"], Some(0)));
  }

  #[test]
  fn should_accept_dotted_and_rust_paths_as_name_paths() {
    assert_eq!(shape("UsersService.findOne"), path(Anchor::Relative, &["UsersService", "findOne"], None));
    assert_eq!(shape("store::InvoiceStore/find"), path(Anchor::Relative, &["store", "InvoiceStore", "find"], None));
    assert_eq!(shape("Store::get"), path(Anchor::Relative, &["Store", "get"], None));
    assert_eq!(
      shape("<PgInvoiceStore as InvoiceStore>::insert"),
      path(Anchor::Relative, &["PgInvoiceStore", "<InvoiceStore>", "insert"], None)
    );
    assert_eq!(shape("MemStore/<Store>/get"), path(Anchor::Relative, &["MemStore", "<Store>", "get"], None));
  }

  #[test]
  fn should_split_on_slashes_before_dots_so_test_names_keep_their_dots() {
    assert_eq!(shape("money/should round 1.5 up"), path(Anchor::Relative, &["money", "should round 1.5 up"], None));
  }

  #[test]
  fn should_keep_brackets_that_are_not_an_overload_index_in_the_name() {
    assert_eq!(shape("Box/[key]"), path(Anchor::Relative, &["Box", "[key]"], None));
    assert_eq!(shape("f[]"), path(Anchor::Relative, &["f[]"], None));
    assert_eq!(shape("f[-1]"), path(Anchor::Relative, &["f[-1]"], None));
  }

  #[test]
  fn should_reject_empty_paths_segments_and_inner_indexes() {
    for text in ["", "   ", "/", "A//b", "A/", "/ /b", "A::::b"] {
      assert!(NamePath::parse(text).is_err(), "{text:?}");
    }
    assert!(NamePath::parse("A[0]/b").is_err());
  }

  #[test]
  fn should_match_a_relative_path_against_the_tail_of_the_qualified_key_path() {
    let qualified = owned(&["src", "users", "users.service", "UsersService", "findOne"]);
    let in_file = owned(&["UsersService", "findOne"]);

    assert!(matches("findOne", &qualified, &in_file, NameMatch::Exact));
    assert!(matches("UsersService/findOne", &qualified, &in_file, NameMatch::Exact));
    assert!(matches("users.service/UsersService/findOne", &qualified, &in_file, NameMatch::Exact));
    assert!(!matches("users/findOne", &qualified, &in_file, NameMatch::Exact));
    assert!(!matches("find", &qualified, &in_file, NameMatch::Exact));
    assert!(matches("UsersService/find", &qualified, &in_file, NameMatch::Substring));
    assert!(!matches("Users/find", &qualified, &in_file, NameMatch::Substring));
  }

  #[test]
  fn should_match_an_absolute_path_only_from_the_top_of_the_file() {
    let qualified = owned(&["consumer", "glob_explicit", "explicit_beats_glob"]);
    let in_file = owned(&["glob_explicit", "explicit_beats_glob"]);

    assert!(matches("/glob_explicit/explicit_beats_glob", &qualified, &in_file, NameMatch::Exact));
    assert!(!matches("/explicit_beats_glob", &qualified, &in_file, NameMatch::Exact));
    assert!(!matches("/consumer/glob_explicit/explicit_beats_glob", &qualified, &in_file, NameMatch::Exact));
  }

  #[test]
  fn should_skip_trait_segments_unless_the_pattern_names_one() {
    let qualified = owned(&["MemStore", "<Store>", "get"]);

    assert!(matches("MemStore/get", &qualified, &qualified, NameMatch::Exact));
    assert!(matches("/MemStore/get", &qualified, &qualified, NameMatch::Exact));
    assert!(matches("<Store>/get", &qualified, &qualified, NameMatch::Exact));
    assert!(matches("MemStore::<Store>::get", &qualified, &qualified, NameMatch::Exact));
    assert!(!matches("Store/get", &qualified, &qualified, NameMatch::Exact));
    assert!(!matches("<Other>/get", &qualified, &qualified, NameMatch::Exact));
  }

  #[test]
  fn should_offer_a_split_relative_path_again_as_one_segment() {
    let dotted = NamePath::parse("should round 1.5 up[1]").unwrap();
    let slashed = NamePath::parse("GET /users/:id").unwrap();
    let fallback = |parsed: &NamePath| parsed.literal_fallback().map(|fallback| (fallback.anchor, fallback.segments, fallback.index));

    assert_eq!(dotted.segments, owned(&["should round 1", "5 up"]));
    assert_eq!(fallback(&dotted), Some(path(Anchor::Relative, &["should round 1.5 up"], Some(1))));
    assert_eq!(slashed.segments, owned(&["GET", "users", ":id"]));
    assert_eq!(fallback(&slashed), Some(path(Anchor::Relative, &["GET /users/:id"], None)));
    assert_eq!(NamePath::parse("/a/b").unwrap().literal_fallback(), None);
    assert_eq!(NamePath::parse("toCents").unwrap().literal_fallback(), None);
  }

  #[test]
  fn should_anchor_crate_and_crate_name_prefixes_at_the_crate_root() {
    let qualified = owned(&["store", "InvoiceStore", "find"]);

    assert!(matches("crate::store::InvoiceStore::find", &qualified, &qualified, NameMatch::Exact));
    assert!(matches("acme_billing::store::InvoiceStore::find", &qualified, &qualified, NameMatch::Exact));
    assert!(matches("crate::store::InvoiceStore/find", &qualified, &qualified, NameMatch::Exact));
    assert!(!matches("crate::InvoiceStore::find", &qualified, &qualified, NameMatch::Exact));
    assert!(!matches("other_crate::store::InvoiceStore::find", &qualified, &qualified, NameMatch::Exact));
  }
}
