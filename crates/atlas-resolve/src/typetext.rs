const DEREF_WRAPPERS: [&str; 3] = ["Box", "Arc", "Rc"];
const AUTO_TRAITS: [&str; 5] = ["Send", "Sync", "Unpin", "Sized", "Copy"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Syntax {
  Rust,
  Ts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TypeShape {
  Path(Vec<String>),
  Bounds(Vec<Vec<String>>),
  Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ValueType {
  Declared(String),
  Constructed(Vec<String>),
}

fn top_level_positions(syntax: Syntax, text: &str, wanted: impl Fn(&str, usize) -> bool) -> Vec<usize> {
  let mut depth = 0i32;
  let mut found = Vec::new();
  let bytes = text.as_bytes();
  let mut quote: Option<u8> = None;
  for (index, &byte) in bytes.iter().enumerate() {
    if let Some(open) = quote {
      if byte == open && (index == 0 || bytes[index - 1] != b'\\') {
        quote = None;
      }
      continue;
    }
    match byte {
      b'"' | b'`' => quote = Some(byte),
      b'\'' if syntax == Syntax::Ts => quote = Some(byte),
      b'(' | b'[' | b'{' | b'<' => depth += 1,
      b')' | b']' | b'}' => depth -= 1,
      b'>' if index > 0 && bytes[index - 1] == b'-' => {}
      b'>' if index > 0 && bytes[index - 1] == b'=' => {}
      b'>' => depth -= 1,
      _ if depth == 0 && wanted(text, index) => found.push(index),
      _ => {}
    }
  }
  found
}

fn split_top_level(syntax: Syntax, text: &str, separator: u8) -> Vec<&str> {
  let mut parts = Vec::new();
  let mut start = 0;
  for index in top_level_positions(syntax, text, |text, index| text.as_bytes()[index] == separator) {
    parts.push(&text[start..index]);
    start = index + 1;
  }
  parts.push(&text[start..]);
  parts.into_iter().map(str::trim).filter(|part| !part.is_empty()).collect()
}

fn is_single_colon(text: &str, index: usize) -> bool {
  let bytes = text.as_bytes();
  bytes[index] == b':' && bytes.get(index + 1) != Some(&b':') && (index == 0 || bytes[index - 1] != b':')
}

fn is_assignment(text: &str, index: usize) -> bool {
  let bytes = text.as_bytes();
  bytes[index] == b'=' && !matches!(bytes.get(index + 1), Some(b'=' | b'>')) && (index == 0 || !matches!(bytes[index - 1], b'=' | b'!' | b'<' | b'>'))
}

fn path_segments(text: &str, separator: &str) -> Option<Vec<String>> {
  let segments: Vec<String> = text.split(separator).map(|segment| segment.trim().to_owned()).collect();
  let valid = segments
    .iter()
    .all(|segment| !segment.is_empty() && segment.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$'));
  valid.then_some(segments)
}

fn matching_close(text: &str, open_index: usize, open: u8, close: u8) -> Option<usize> {
  let mut depth = 0i32;
  let bytes = text.as_bytes();
  for (index, &byte) in bytes.iter().enumerate().skip(open_index) {
    let arrow = byte == b'>' && index > 0 && matches!(bytes[index - 1], b'-' | b'=');
    if byte == open {
      depth += 1;
    } else if byte == close && !arrow {
      depth -= 1;
      if depth == 0 {
        return Some(index);
      }
    }
  }
  None
}

pub(crate) fn rust_declared_type(signature: &str) -> Option<String> {
  let colon = *top_level_positions(Syntax::Rust, signature, is_single_colon).first()?;
  let rest = &signature[colon + 1..];
  let end = top_level_positions(Syntax::Rust, rest, is_assignment).first().copied().unwrap_or(rest.len());
  let text = rest[..end].trim();
  (!text.is_empty()).then(|| text.to_owned())
}

pub(crate) fn rust_return_type(signature: &str) -> Option<String> {
  if signature.ends_with('…') {
    return None;
  }
  let bytes = signature.as_bytes();
  let is_where = |text: &str, index: usize| text.as_bytes()[index..].starts_with(b" where ");
  let head = match top_level_positions(Syntax::Rust, signature, is_where).first() {
    Some(at) => &signature[..*at],
    None => signature,
  };
  let arrow = top_level_positions(Syntax::Rust, head, |_, index| bytes[index] == b'-' && bytes.get(index + 1) == Some(&b'>'))
    .into_iter()
    .next()?;
  let text = head[arrow + 2..].trim();
  (!text.is_empty()).then(|| text.to_owned())
}

pub(crate) fn rust_alias_target(signature: &str) -> Option<String> {
  if signature.ends_with('…') {
    return None;
  }
  let equals = *top_level_positions(Syntax::Rust, signature, is_assignment).first()?;
  let text = signature[equals + 1..].trim();
  (!text.is_empty()).then(|| text.to_owned())
}

pub(crate) fn rust_type_parts(text: &str) -> Option<(Vec<String>, Vec<&str>)> {
  let text = strip_reference(text);
  let (head, args) = match text.find('<') {
    Some(open) => {
      let close = matching_close(text, open, b'<', b'>')?;
      if !text[close + 1..].trim().is_empty() {
        return None;
      }
      (&text[..open], split_top_level(Syntax::Rust, &text[open + 1..close], b','))
    }
    None => (text, Vec::new()),
  };
  let path = path_segments(head, "::")?;
  let args = args.into_iter().filter(|arg| !arg.starts_with('\'')).collect();
  Some((path, args))
}

fn generic_list_after<'a>(signature: &'a str, name: &str) -> Option<&'a str> {
  let bytes = signature.as_bytes();
  let open = signature.match_indices(name).find_map(|(index, _)| {
    let boundary = index == 0 || !(bytes[index - 1].is_ascii_alphanumeric() || bytes[index - 1] == b'_');
    let open = index + name.len();
    (boundary && bytes.get(open) == Some(&b'<')).then_some(open)
  })?;
  let close = matching_close(signature, open, b'<', b'>')?;
  Some(&signature[open + 1..close])
}

pub(crate) fn ts_type_params(signature: &str, name: &str) -> Vec<String> {
  let Some(list) = generic_list_after(signature, name) else { return Vec::new() };
  split_top_level(Syntax::Ts, list, b',')
    .into_iter()
    .filter_map(|parameter| {
      let head: String = parameter
        .trim_start_matches("const ")
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
        .collect();
      (!head.is_empty()).then_some(head)
    })
    .collect()
}

pub(crate) fn rust_generic_bounds(signature: &str, name: &str) -> Vec<(String, Vec<String>)> {
  let mut bounds: Vec<(String, Vec<String>)> = Vec::new();
  let mut add = |name: &str, list: &str| {
    let name = name.trim();
    if name.is_empty() || name.starts_with('\'') || name.starts_with("const ") {
      return;
    }
    let traits: Vec<String> = split_top_level(Syntax::Rust, list, b'+')
      .into_iter()
      .filter(|bound| !bound.starts_with('\'') && !bound.starts_with('?'))
      .map(str::to_owned)
      .collect();
    match bounds.iter_mut().find(|(existing, _)| existing == name) {
      Some((_, existing)) => existing.extend(traits),
      None => bounds.push((name.to_owned(), traits)),
    }
  };
  if let Some(list) = generic_list_after(signature, name) {
    for parameter in split_top_level(Syntax::Rust, list, b',') {
      match parameter.split_once(':') {
        Some((generic, list)) => add(generic, list),
        None => add(parameter.split('=').next().unwrap_or(parameter), ""),
      }
    }
  }
  if let Some(where_index) = signature.find(" where ") {
    for clause in split_top_level(Syntax::Rust, &signature[where_index + 7..], b',') {
      if let Some((name, list)) = clause.split_once(':') {
        add(name, list);
      }
    }
  }
  bounds
}

pub(crate) fn strip_reference(text: &str) -> &str {
  let mut text = text.trim();
  loop {
    let before = text;
    if let Some(rest) = text.strip_prefix('&') {
      text = rest.trim_start();
      if text.starts_with('\'') {
        text = text.split_once(' ').map_or("", |(_, rest)| rest).trim_start();
      }
    }
    if let Some(rest) = text.strip_prefix("mut ") {
      text = rest.trim_start();
    }
    if text == before {
      return text;
    }
  }
}

fn bound_paths(list: &str) -> Vec<Vec<String>> {
  split_top_level(Syntax::Rust, list, b'+')
    .into_iter()
    .filter(|bound| !bound.starts_with('\'') && !bound.starts_with('?'))
    .filter_map(|bound| {
      let head = bound.split('<').next().unwrap_or(bound);
      path_segments(head, "::")
    })
    .filter(|path| path.last().is_none_or(|last| !AUTO_TRAITS.contains(&last.as_str())))
    .collect()
}

pub(crate) fn rust_type_shape(text: &str) -> TypeShape {
  let mut text = strip_reference(text);
  for _ in 0..8 {
    if let Some(rest) = text.strip_prefix("dyn ").or_else(|| text.strip_prefix("impl ")) {
      let bounds = bound_paths(rest);
      return if bounds.is_empty() { TypeShape::Unknown } else { TypeShape::Bounds(bounds) };
    }
    let (head, args) = match text.find('<') {
      Some(open) => {
        let Some(close) = matching_close(text, open, b'<', b'>') else {
          return TypeShape::Unknown;
        };
        (&text[..open], Some(&text[open + 1..close]))
      }
      None => (text, None),
    };
    let Some(path) = path_segments(head, "::") else { return TypeShape::Unknown };
    let wrapper = path.last().is_some_and(|last| DEREF_WRAPPERS.contains(&last.as_str()));
    match (wrapper, args) {
      (true, Some(args)) => {
        let Some(inner) = split_top_level(Syntax::Rust, args, b',').into_iter().next() else {
          return TypeShape::Unknown;
        };
        text = strip_reference(inner);
      }
      _ => return TypeShape::Path(path),
    }
  }
  TypeShape::Unknown
}

pub(crate) fn rust_outer_path(text: &str) -> Option<Vec<String>> {
  let text = strip_reference(text);
  let head = text.split('<').next().unwrap_or(text);
  path_segments(head, "::")
}

pub(crate) fn rust_generic_param(text: &str) -> Option<&str> {
  let text = strip_reference(text);
  let valid = !text.is_empty() && text.chars().all(|c| c.is_alphanumeric() || c == '_');
  valid.then_some(text)
}

pub(crate) fn is_deref_wrapper(path: &[String]) -> bool {
  path.last().is_some_and(|last| DEREF_WRAPPERS.contains(&last.as_str()))
}

pub(crate) fn ts_value_type(signature: &str) -> Option<ValueType> {
  let positions = top_level_positions(Syntax::Ts, signature, |text, index| is_single_colon(text, index) || is_assignment(text, index));
  let first = *positions.first()?;
  if signature.as_bytes()[first] == b':' {
    let rest = &signature[first + 1..];
    let end = top_level_positions(Syntax::Ts, rest, is_assignment).first().copied().unwrap_or(rest.len());
    let text = rest[..end].trim();
    return (!text.is_empty()).then(|| ValueType::Declared(text.to_owned()));
  }
  let initializer = signature[first + 1..].trim();
  let constructor = initializer.strip_prefix("new ")?;
  let end = constructor.find(['(', '<']).unwrap_or(constructor.len());
  path_segments(&constructor[..end], ".").map(ValueType::Constructed)
}

pub(crate) fn ts_return_type(signature: &str) -> Option<String> {
  let open = signature.find('(')?;
  let close = matching_close(signature, open, b'(', b')')?;
  let rest = signature[close + 1..].trim_start().strip_prefix(':')?;
  let rest = rest.split("=>").next().unwrap_or(rest);
  let text = rest.trim().trim_end_matches('{').trim();
  (!text.is_empty()).then(|| text.to_owned())
}

pub(crate) fn ts_type_head(text: &str) -> Option<Vec<String>> {
  let text = text.trim();
  let text = text.strip_prefix("readonly ").unwrap_or(text);
  let alternatives: Vec<&str> = split_top_level(Syntax::Ts, text, b'|')
    .into_iter()
    .filter(|part| !matches!(*part, "undefined" | "null"))
    .collect();
  let [single] = alternatives.as_slice() else { return None };
  if split_top_level(Syntax::Ts, single, b'&').len() != 1 || single.ends_with(']') {
    return None;
  }
  let head = single.split('<').next().unwrap_or(single);
  path_segments(head, ".")
}

#[cfg(test)]
mod tests {
  use super::*;

  fn path(segments: &[&str]) -> TypeShape {
    TypeShape::Path(segments.iter().map(|segment| (*segment).to_owned()).collect())
  }

  fn bounds(paths: &[&[&str]]) -> TypeShape {
    TypeShape::Bounds(paths.iter().map(|path| path.iter().map(|segment| (*segment).to_owned()).collect()).collect())
  }

  #[test]
  fn should_extract_declared_types_from_rust_field_and_const_signatures() {
    assert_eq!(rust_declared_type("pub store: Arc<dyn InvoiceStore>").as_deref(), Some("Arc<dyn InvoiceStore>"));
    assert_eq!(rust_declared_type("entries: HashMap<String, String>").as_deref(), Some("HashMap<String, String>"));
    assert_eq!(rust_declared_type("pub const LIMIT: usize").as_deref(), Some("usize"));
    assert_eq!(rust_declared_type("pub(in crate::a) inner: a::Thing").as_deref(), Some("a::Thing"));
    assert_eq!(rust_declared_type("static CONFIG: Config = Config::new()").as_deref(), Some("Config"));
    assert_eq!(rust_declared_type("A"), None);
  }

  #[test]
  fn should_extract_rust_return_types_without_where_clauses() {
    assert_eq!(rust_return_type("pub fn new(root: &str) -> Self").as_deref(), Some("Self"));
    assert_eq!(rust_return_type("fn f<T>(t: T) -> Vec<T> where T: Clone").as_deref(), Some("Vec<T>"));
    assert_eq!(rust_return_type("fn f()"), None);
    assert_eq!(rust_return_type("fn f(cb: impl Fn() -> u8) -> Store").as_deref(), Some("Store"));
    assert_eq!(rust_return_type("fn f<F: Fn() -> u8>(cb: F)"), None);
    assert_eq!(rust_return_type("fn f<F>(cb: F) where F: Fn() -> u8"), None);
    assert_eq!(rust_return_type("fn f() -> impl Fn() -> Store").as_deref(), Some("impl Fn() -> Store"));
    assert_eq!(rust_return_type("fn f() -> Box<dyn Fn(u8) -> Store>").as_deref(), Some("Box<dyn Fn(u8) -> Store>"));
    assert_eq!(rust_return_type("fn f(a: A, b: B…"), None);
  }

  #[test]
  fn should_read_alias_targets_and_type_arguments() {
    assert_eq!(
      rust_alias_target("pub type StoreResult<T> = Result<T, StoreError>").as_deref(),
      Some("Result<T, StoreError>")
    );
    assert_eq!(rust_alias_target("type Map<K, V = u8> = HashMap<K, V>").as_deref(), Some("HashMap<K, V>"));
    assert_eq!(rust_alias_target("type Output;"), None);
    assert_eq!(rust_alias_target("type Long = Vec<…"), None);
    assert_eq!(rust_type_parts("&'a MutexGuard<'_, Budget>"), Some((vec!["MutexGuard".to_owned()], vec!["Budget"])));
    assert_eq!(
      rust_type_parts("crate::store::Tx<'c, Vec<u8>, E>"),
      Some((vec!["crate".to_owned(), "store".to_owned(), "Tx".to_owned()], vec!["Vec<u8>", "E"]))
    );
    assert_eq!(rust_type_parts("(A, B)"), None);
    assert_eq!(rust_type_parts("<T as Trait>::Out"), None);
    assert_eq!(rust_type_parts("Vec<u8>::Item"), None);
  }

  #[test]
  fn should_collect_generic_bounds_from_parameters_and_where_clauses() {
    let parsed = rust_generic_bounds("pub(crate) fn f<'a, T: Clone + Send, U, const N: usize>(t: &T, u: U) where U: Iterator + 'a", "f");
    assert_eq!(
      parsed,
      vec![("T".to_owned(), vec!["Clone".to_owned(), "Send".to_owned()]), ("U".to_owned(), vec!["Iterator".to_owned()])]
    );
    assert_eq!(
      rust_generic_bounds("pub fn read_generic<T: Store>(store: &T, key: &str) -> Option<String>", "read_generic")[0].1,
      ["Store"]
    );
    assert!(rust_generic_bounds("fn plain(x: Vec<u8>)", "plain").is_empty());
    assert_eq!(
      rust_generic_bounds("pub struct Svc<R: Repo, S = Box<dyn Fn() -> u8>>", "Svc"),
      vec![("R".to_owned(), vec!["Repo".to_owned()]), ("S".to_owned(), Vec::new())]
    );
    assert!(rust_generic_bounds("pub struct MySvc<R>", "Svc").is_empty());
  }

  #[test]
  fn should_read_ts_type_parameters_of_the_named_declaration() {
    assert_eq!(ts_type_params("class Base<Model extends { save(): void }, K = string>", "Base"), ["Model", "K"]);
    assert_eq!(ts_type_params("go<T>(x: T): T", "go"), ["T"]);
    assert!(ts_type_params("class A extends B<X>", "A").is_empty());
  }

  #[test]
  fn should_see_through_references_and_deref_wrappers() {
    assert_eq!(rust_type_shape("&MemStore"), path(&["MemStore"]));
    assert_eq!(rust_type_shape("&'a mut crate::a::Thing<u8>"), path(&["crate", "a", "Thing"]));
    assert_eq!(rust_type_shape("Box<dyn Store>"), bounds(&[&["Store"]]));
    assert_eq!(rust_type_shape("Arc<dyn InvoiceStore + Send + Sync>"), bounds(&[&["InvoiceStore"]]));
    assert_eq!(rust_type_shape("Rc<Box<Inner>>"), path(&["Inner"]));
    assert_eq!(rust_type_shape("impl Store"), bounds(&[&["Store"]]));
    assert_eq!(rust_type_shape("&dyn Send"), TypeShape::Unknown);
    assert_eq!(rust_type_shape("(u8, u8)"), TypeShape::Unknown);
    assert_eq!(rust_type_shape("HashMap<String, String>"), path(&["HashMap"]));
  }

  #[test]
  fn should_read_ts_annotations_and_constructor_initialisers() {
    assert_eq!(
      ts_value_type("private readonly usersService: UsersService"),
      Some(ValueType::Declared("UsersService".into()))
    );
    assert_eq!(ts_value_type("public other?: Other"), Some(ValueType::Declared("Other".into())));
    assert_eq!(
      ts_value_type("private cache: Map<string, string> = new Map()"),
      Some(ValueType::Declared("Map<string, string>".into()))
    );
    assert_eq!(
      ts_value_type("const acme = new AcmeClient({ baseUrl: API_URL, auth: { type: 'bearer' } })"),
      Some(ValueType::Constructed(vec!["AcmeClient".into()]))
    );
    assert_eq!(
      ts_value_type("field = new sdk.Client<T>()"),
      Some(ValueType::Constructed(vec!["sdk".into(), "Client".into()]))
    );
    assert_eq!(ts_value_type("let x = cond ? a : b"), None);
    assert_eq!(ts_value_type("const f = (a: number) => a"), None);
    assert_eq!(ts_value_type("repo: Repo"), Some(ValueType::Declared("Repo".into())));
  }

  #[test]
  fn should_read_ts_return_types_and_type_heads() {
    assert_eq!(ts_return_type("async getUser(id: string): Promise<User>").as_deref(), Some("Promise<User>"));
    assert_eq!(ts_return_type("get size(): number").as_deref(), Some("number"));
    assert_eq!(ts_return_type("findOne(id: string)"), None);
    assert_eq!(ts_type_head("Repository<User>"), Some(vec!["Repository".to_owned()]));
    assert_eq!(ts_type_head("UsersService | undefined"), Some(vec!["UsersService".to_owned()]));
    assert_eq!(ts_type_head("sdk.Client"), Some(vec!["sdk".to_owned(), "Client".to_owned()]));
    assert_eq!(ts_type_head("A | B"), None);
    assert_eq!(ts_type_head("User[]"), None);
    assert_eq!(ts_type_head("{ a: string }"), None);
  }
}
