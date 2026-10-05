use std::collections::{BTreeSet, HashMap, HashSet};

use atlas_core::EntityId;

use crate::QueryEngine;
use crate::graph::FileKey;

const COMMON_NAMES: [&str; 64] = [
  "new",
  "get",
  "set",
  "run",
  "default",
  "from",
  "into",
  "len",
  "is_empty",
  "clone",
  "fmt",
  "build",
  "parse",
  "name",
  "id",
  "with",
  "add",
  "insert",
  "remove",
  "update",
  "find",
  "apply",
  "init",
  "start",
  "stop",
  "next",
  "load",
  "save",
  "read",
  "write",
  "open",
  "close",
  "handle",
  "process",
  "call",
  "execute",
  "create",
  "delete",
  "list",
  "map",
  "value",
  "key",
  "data",
  "to_string",
  "as_str",
  "eq",
  "hash",
  "drop",
  "deref",
  "index",
  "iter",
  "push",
  "pop",
  "contains",
  "reset",
  "clear",
  "validate",
  "check",
  "render",
  "emit",
  "send",
  "toString",
  "then",
  "on",
];

pub(crate) struct TextCheck {
  pub index: usize,
  pub name: String,
  pub owner: Option<String>,
  pub file: FileKey,
  pub span: (u32, u32),
  pub package: Option<EntityId>,
  pub dependents: Vec<EntityId>,
  pub skip_test_code: bool,
}

#[derive(Default, Clone)]
pub(crate) struct TestCode {
  pub files: HashSet<FileKey>,
  pub ranges: HashMap<FileKey, Vec<(u32, u32)>>,
}

impl TestCode {
  pub fn contains(&self, file: &FileKey, line: u32) -> bool {
    self.files.contains(file)
      || atlas_core::is_test_path(file.1.as_str())
      || self
        .ranges
        .get(file)
        .is_some_and(|ranges| ranges.iter().any(|(start, end)| (*start..=*end).contains(&line)))
  }
}

#[derive(Default)]
pub(crate) struct TextScopes {
  pub packages: HashMap<EntityId, Vec<FileKey>>,
  pub macro_files: BTreeSet<FileKey>,
  pub test_code: TestCode,
}

struct Occurrence {
  file: usize,
  line: u32,
  line_start: usize,
  line_end: usize,
  column: usize,
}

pub(crate) struct TextIndex {
  files: Vec<FileKey>,
  position: HashMap<FileKey, usize>,
  texts: Vec<Option<String>>,
  unreadable: Vec<usize>,
  macro_mask: Vec<bool>,
  package_masks: HashMap<EntityId, Vec<bool>>,
  test_code: TestCode,
  occurrences: HashMap<String, Vec<Occurrence>>,
}

struct LineVerdict {
  macro_call: bool,
  found: bool,
}

impl TextIndex {
  pub fn build(query: &QueryEngine, checks: &[TextCheck], scopes: &TextScopes) -> Self {
    Self::from_reader(checks, scopes, |(repo, path)| query.engine().read_source(repo, path).ok().map(|(text, _)| text))
  }

  fn from_reader(checks: &[TextCheck], scopes: &TextScopes, read: impl Fn(&FileKey) -> Option<String>) -> Self {
    let mut union: BTreeSet<&FileKey> = BTreeSet::new();
    if !checks.is_empty() {
      union.extend(scopes.macro_files.iter());
    }
    let mut packages: BTreeSet<EntityId> = BTreeSet::new();
    for check in checks {
      union.insert(&check.file);
      packages.extend(check.package);
      packages.extend(check.dependents.iter().copied());
    }
    for package in &packages {
      union.extend(scopes.packages.get(package).into_iter().flatten());
    }
    let files: Vec<FileKey> = union.into_iter().cloned().collect();
    let position: HashMap<FileKey, usize> = files.iter().enumerate().map(|(index, file)| (file.clone(), index)).collect();
    let mask = |members: &mut dyn Iterator<Item = &FileKey>| {
      let mut mask = vec![false; files.len()];
      for file in members {
        if let Some(&index) = position.get(file) {
          mask[index] = true;
        }
      }
      mask
    };
    let macro_mask = mask(&mut scopes.macro_files.iter());
    let package_masks = packages
      .iter()
      .map(|package| (*package, mask(&mut scopes.packages.get(package).into_iter().flatten())))
      .collect();
    let texts: Vec<Option<String>> = files.iter().map(read).collect();
    let unreadable = texts.iter().enumerate().filter(|(_, text)| text.is_none()).map(|(index, _)| index).collect();
    let wanted: HashSet<&str> = checks.iter().map(|check| check.name.as_str()).filter(|name| is_word_like(name)).collect();
    let mut occurrences: HashMap<String, Vec<Occurrence>> = HashMap::new();
    for (file, text) in texts.iter().enumerate() {
      let Some(text) = text else { continue };
      for (index, line) in text.lines().enumerate() {
        let line_start = line.as_ptr() as usize - text.as_ptr() as usize;
        for (column, token) in words(line) {
          if !wanted.contains(token) {
            continue;
          }
          occurrences.entry(token.to_owned()).or_default().push(Occurrence {
            file,
            line: index as u32 + 1,
            line_start,
            line_end: line_start + line.len(),
            column,
          });
        }
      }
    }
    Self {
      files,
      position,
      texts,
      unreadable,
      macro_mask,
      package_masks,
      test_code: scopes.test_code.clone(),
      occurrences,
    }
  }

  pub fn verify(&self, check: &TextCheck) -> Option<String> {
    let own = self.position.get(&check.file).copied();
    let masks: Vec<&Vec<bool>> = check
      .package
      .iter()
      .chain(&check.dependents)
      .filter_map(|package| self.package_masks.get(package))
      .collect();
    let in_scope = |file: usize| Some(file) == own || self.macro_mask[file] || masks.iter().any(|mask| mask[file]);
    let matcher = Matcher::new(check);
    let mut macro_hit: Option<(usize, u32)> = None;
    let mut first_hit: Option<(usize, u32)> = None;
    let mut hits = 0;
    let mut record = |file: usize, number: u32, line: &str, starts: &[usize]| {
      if Some(file) == own && (check.span.0..=check.span.1).contains(&number) {
        return;
      }
      let kept;
      let mut starts = starts;
      if check.skip_test_code {
        if self.test_code.contains(&self.files[file], number) || is_non_use_line(line) {
          return;
        }
        let self_type = impl_self_type_offset(line);
        kept = starts
          .iter()
          .copied()
          .filter(|start| self_type.is_none_or(|offset| *start < offset))
          .collect::<Vec<usize>>();
        if kept.is_empty() {
          return;
        }
        starts = kept.as_slice();
      }
      let verdict = matcher.judge(line, starts, Some(file) == own, self.macro_mask[file]);
      if verdict.macro_call && macro_hit.is_none() {
        macro_hit = Some((file, number));
      }
      if verdict.found {
        hits += 1;
        first_hit.get_or_insert((file, number));
      }
    };
    match is_word_like(&check.name) {
      true => {
        let occurrences = self.occurrences.get(&check.name).map_or(&[][..], Vec::as_slice);
        let mut columns: Vec<usize> = Vec::new();
        for (index, occurrence) in occurrences.iter().enumerate() {
          if !in_scope(occurrence.file) {
            continue;
          }
          columns.push(occurrence.column);
          let closes = occurrences.get(index + 1).is_none_or(|next| (next.file, next.line) != (occurrence.file, occurrence.line));
          if closes {
            let text = self.texts[occurrence.file].as_deref().unwrap_or_default();
            record(occurrence.file, occurrence.line, &text[occurrence.line_start..occurrence.line_end], &columns);
            columns.clear();
          }
        }
      }
      false => {
        for (file, text) in self.texts.iter().enumerate() {
          let Some(text) = text.as_deref().filter(|_| in_scope(file)) else { continue };
          for (index, line) in text.lines().enumerate() {
            let starts = bounded(line, &check.name);
            if !starts.is_empty() {
              record(file, index as u32 + 1, line, &starts);
            }
          }
        }
      }
    }
    if let Some(location) = macro_hit {
      return Some(format!(
        "called by name inside a macro_rules!/include! file ({}); macro-expanded callers are not indexed",
        self.location(location)
      ));
    }
    if let Some(location) = first_hit {
      let form = match COMMON_NAMES.contains(&check.name.as_str()) {
        true => "its qualified name",
        false => "its name",
      };
      return Some(format!(
        "{form} occurs {hits}x in its package (first at {}) although no reference was recorded",
        self.location(location)
      ));
    }
    let unreadable: Vec<usize> = self.unreadable.iter().copied().filter(|file| in_scope(*file)).collect();
    let first = unreadable.first()?;
    let (repo, path) = &self.files[*first];
    Some(format!("{} files in scope could not be read for a textual check (first: {repo}:{path})", unreadable.len()))
  }

  fn location(&self, (file, line): (usize, u32)) -> String {
    let (repo, path) = &self.files[file];
    format!("{repo}:{path}:{line}")
  }
}

struct Matcher<'c> {
  name: &'c str,
  owner_receivers: Option<[String; 2]>,
  common: bool,
}

impl<'c> Matcher<'c> {
  fn new(check: &'c TextCheck) -> Self {
    Self {
      name: &check.name,
      owner_receivers: check.owner.as_ref().map(|owner| [format!("{owner}::"), format!("{owner}.")]),
      common: COMMON_NAMES.contains(&check.name.as_str()),
    }
  }

  fn judge(&self, line: &str, starts: &[usize], own_file: bool, macro_file: bool) -> LineVerdict {
    let macro_call = macro_file && starts.iter().any(|start| self.is_call(line, *start));
    let found = match (&self.owner_receivers, self.common) {
      (_, false) => !starts.is_empty(),
      (Some(receivers), true) => starts.iter().any(|start| {
        let before = &line[..*start];
        receivers.iter().any(|receiver| before.ends_with(receiver.as_str())) || (own_file && ["Self::", "self.", "this."].iter().any(|receiver| before.ends_with(receiver)))
      }),
      (None, true) => false,
    };
    LineVerdict { macro_call, found }
  }

  fn is_call(&self, line: &str, start: usize) -> bool {
    let before = line[..start].trim_end();
    let after = line[start + self.name.len()..].trim_start();
    let definition = before.ends_with("fn") || before.ends_with("function");
    after.starts_with('(') && !definition
  }
}

fn is_non_use_line(line: &str) -> bool {
  let line = line.trim_start();
  if line.starts_with("//") || line.starts_with("/*") || line.starts_with("*/") || line == "*" || line.starts_with("* ") {
    return true;
  }
  if ["import ", "import{", "import\t"].iter().any(|prefix| line.starts_with(prefix)) {
    return true;
  }
  if line.starts_with("export ") && (line.starts_with("export {") || line.starts_with("export *") || line.contains(" from ")) {
    return true;
  }
  let declaration = strip_visibility(line);
  declaration.starts_with("use ") || (declaration.starts_with("mod ") && declaration.trim_end().ends_with(';'))
}

fn strip_visibility(line: &str) -> &str {
  let Some(rest) = line.strip_prefix("pub") else { return line };
  if let Some(restricted) = rest.strip_prefix('(') {
    return restricted.split_once(')').map_or(line, |(_, after)| after.trim_start());
  }
  rest.trim_start()
}

fn impl_self_type_offset(line: &str) -> Option<usize> {
  let trimmed = line.trim_start();
  let rest = trimmed.strip_prefix("impl").filter(|rest| rest.starts_with(|c: char| c.is_whitespace() || c == '<'))?;
  let base = line.len() - rest.len();
  let mut depth = 0usize;
  let mut generics_end = 0;
  let mut previous = ' ';
  for (index, character) in rest.char_indices() {
    match character {
      '<' => depth += 1,
      '>' if previous != '-' => depth = depth.saturating_sub(1),
      _ => {}
    }
    previous = character;
    if depth == 0 && index > 0 && character == '>' {
      generics_end = index + 1;
      break;
    }
    if depth == 0 && !character.is_whitespace() && character != '<' {
      break;
    }
  }
  let mut depth = 0usize;
  let bytes = rest.as_bytes();
  for (index, character) in rest.char_indices().skip_while(|(index, _)| *index < generics_end) {
    match character {
      '<' | '(' | '[' => depth += 1,
      '>' | ')' | ']' => depth = depth.saturating_sub(1),
      ' ' if depth == 0 && bytes[index..].starts_with(b" for ") => return Some(base + index + " for ".len()),
      _ => {}
    }
  }
  Some(base + generics_end)
}

fn is_word(c: char) -> bool {
  c.is_alphanumeric() || c == '_' || c == '$'
}

fn is_word_like(name: &str) -> bool {
  !name.is_empty() && name.chars().all(is_word)
}

fn words(line: &str) -> impl Iterator<Item = (usize, &str)> {
  let mut chars = line.char_indices().peekable();
  std::iter::from_fn(move || {
    let start = loop {
      let (index, c) = chars.next()?;
      if is_word(c) {
        break index;
      }
    };
    let mut end = line.len();
    while let Some(&(index, c)) = chars.peek() {
      if !is_word(c) {
        end = index;
        break;
      }
      chars.next();
    }
    Some((start, &line[start..end]))
  })
}

fn bounded(line: &str, word: &str) -> Vec<usize> {
  if word.is_empty() {
    return Vec::new();
  }
  line
    .match_indices(word)
    .filter(|(start, _)| {
      let before = line[..*start].chars().next_back();
      let after = line[start + word.len()..].chars().next();
      !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
    .map(|(start, _)| start)
    .collect()
}

#[cfg(test)]
mod tests {
  use atlas_core::{RelPath, RepoName};

  use super::*;

  #[test]
  fn should_split_lines_into_maximal_word_runs() {
    let found: Vec<(usize, &str)> = words("let a_b = $c9(1) + é2;").collect();
    assert_eq!(found, vec![(0, "let"), (4, "a_b"), (10, "$c9"), (14, "1"), (19, "é2")]);
  }

  #[test]
  fn should_find_the_same_bounded_occurrences_as_maximal_word_runs() {
    let lines = ["foo(foo_bar, foo) + xfoo + foo$", "  foo::foo.foo", "afoo foo", "", "foo"];
    for line in lines {
      let runs: Vec<usize> = words(line).filter(|(_, word)| *word == "foo").map(|(start, _)| start).collect();
      assert_eq!(runs, bounded(line, "foo"), "{line:?}");
    }
  }

  fn key(path: &str) -> FileKey {
    (RepoName::new("app").unwrap(), RelPath::new(path).unwrap())
  }

  fn reference_verify(check: &TextCheck, scopes: &TextScopes, texts: &HashMap<FileKey, Option<String>>) -> Option<String> {
    let mut scope: BTreeSet<FileKey> = check
      .package
      .and_then(|package| scopes.packages.get(&package))
      .cloned()
      .unwrap_or_default()
      .into_iter()
      .collect();
    scope.insert(check.file.clone());
    scope.extend(scopes.macro_files.iter().cloned());
    let common = COMMON_NAMES.contains(&check.name.as_str());
    let (mut unreadable, mut hits, mut macro_hits) = (Vec::new(), Vec::new(), Vec::new());
    for file in &scope {
      let (repo, path) = file;
      let Some(text) = texts.get(file).cloned().flatten() else {
        unreadable.push(format!("{repo}:{path}"));
        continue;
      };
      let own_file = *file == check.file;
      for (index, line) in text.lines().enumerate() {
        let number = index as u32 + 1;
        if own_file && (check.span.0..=check.span.1).contains(&number) {
          continue;
        }
        let starts = bounded(line, &check.name);
        let call = |start: &usize| {
          let before = line[..*start].trim_end();
          let after = line[start + check.name.len()..].trim_start();
          after.starts_with('(') && !(before.ends_with("fn") || before.ends_with("function"))
        };
        if scopes.macro_files.contains(file) && starts.iter().any(call) {
          macro_hits.push(format!("{repo}:{path}:{number}"));
        }
        let found = match (&check.owner, common) {
          (_, false) => !starts.is_empty(),
          (Some(owner), true) => {
            let mut receivers = vec![format!("{owner}::"), format!("{owner}.")];
            if own_file {
              receivers.extend(["Self::".to_owned(), "self.".to_owned(), "this.".to_owned()]);
            }
            starts.iter().any(|start| receivers.iter().any(|receiver| line[..*start].ends_with(receiver.as_str())))
          }
          (None, true) => false,
        };
        if found {
          hits.push(format!("{repo}:{path}:{number}"));
        }
      }
    }
    if let Some(location) = macro_hits.first() {
      return Some(format!(
        "called by name inside a macro_rules!/include! file ({location}); macro-expanded callers are not indexed"
      ));
    }
    if let Some(location) = hits.first() {
      let form = if common { "its qualified name" } else { "its name" };
      return Some(format!(
        "{form} occurs {}x in its package (first at {location}) although no reference was recorded",
        hits.len()
      ));
    }
    let first = unreadable.first()?;
    Some(format!("{} files in scope could not be read for a textual check (first: {first})", unreadable.len()))
  }

  #[test]
  fn should_verify_exactly_like_a_full_line_scan_on_a_generated_corpus() {
    let fragments = [
      "load",
      "Store::load(",
      "Store.load",
      "self.load",
      "this.load",
      "Self::load",
      "fn load(",
      "function load (",
      "loader",
      "load_all",
      "xload",
      "parse_packet",
      "parse_packet (",
      "Store::new",
      "new(",
      "this.new",
      "#secret",
      "#secret()",
      "é_load",
      "$load",
      " ",
      "\t",
      "(",
      ")",
      ";",
      "::",
      ".",
      "load(",
      "\r",
    ];
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = |bound: usize| {
      seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
      (seed >> 33) as usize % bound
    };
    let paths = ["src/a.rs", "src/b.rs", "src/c.ts", "lib/d.ts", "src/macros.rs", "src/gone.rs"];
    let mut texts: HashMap<FileKey, Option<String>> = HashMap::new();
    for path in paths {
      let text = (0..40)
        .map(|_| (0..next(9)).map(|_| fragments[next(fragments.len())]).collect::<Vec<_>>().join(""))
        .collect::<Vec<_>>()
        .join("\n");
      let text = match path {
        "src/macros.rs" => "macro_rules! run {\n  () => { load() };\n}\nfn load() {}".to_owned(),
        _ => text,
      };
      texts.insert(key(path), (path != "src/gone.rs").then_some(text));
    }
    let package = EntityId::from_i64(42);
    let scopes = TextScopes {
      packages: HashMap::from([(package, vec![key("src/a.rs"), key("src/b.rs"), key("src/gone.rs"), key("src/macros.rs")])]),
      macro_files: BTreeSet::from([key("src/macros.rs")]),
      test_code: TestCode::default(),
    };
    let names = ["load", "parse_packet", "#secret", "loader", "new", ""];
    let owners = [None, Some("Store"), Some("Other")];
    let mut checks = Vec::new();
    for (index, path) in ["src/a.rs", "src/c.ts", "lib/d.ts"].iter().enumerate() {
      for name in names {
        for owner in owners {
          let start = next(30) as u32;
          checks.push(TextCheck {
            index: checks.len(),
            name: name.to_owned(),
            owner: owner.map(str::to_owned),
            file: key(path),
            span: (start, start + next(10) as u32),
            package: (index != 2).then_some(package),
            dependents: Vec::new(),
            skip_test_code: false,
          });
        }
      }
    }
    let index = TextIndex::from_reader(&checks, &scopes, |file| texts.get(file).cloned().flatten());

    for check in &checks {
      assert_eq!(
        index.verify(check),
        reference_verify(check, &scopes, &texts),
        "{} owner {:?} in {:?}",
        check.name,
        check.owner,
        check.file
      );
    }
    let reasons: Vec<String> = checks.iter().filter_map(|check| index.verify(check)).collect();
    for kind in ["called by name", "its qualified name", "its name", "1 files in scope could not be read"] {
      assert!(reasons.iter().any(|reason| reason.starts_with(kind)), "no {kind:?} verdict in {reasons:#?}");
    }
  }

  #[test]
  fn should_skip_only_lines_that_can_never_use_a_name() {
    for line in [
      "import { foo } from './foo';",
      "  export { foo } from './foo';",
      "export * from './foo';",
      "export type { Foo } from './foo';",
      "use crate::foo::Foo;",
      "pub(crate) use super::foo;",
      "pub mod foo;",
      "// foo()",
      "/// foo",
      " * foo",
      "/* foo */",
    ] {
      assert!(is_non_use_line(line), "{line}");
    }
    for line in [
      "foo();",
      "import('./foo')",
      "export const x = foo();",
      "mod foo { fn a() {} }",
      "const s = 'foo';",
      "let a = b * foo;",
    ] {
      assert!(!is_non_use_line(line), "{line}");
    }
  }

  #[test]
  fn should_find_where_the_self_type_of_an_impl_header_starts() {
    let line = "impl From<Foo> for Bar {";
    assert_eq!(impl_self_type_offset(line), line.find("Bar"));
    let line = "impl<T: From<Foo>> Wrapper<T> {";
    assert_eq!(impl_self_type_offset(line), Some(18));
    assert!(line.find("Foo").unwrap() < 18);
    let line = "impl<T> Trait for Vec<T> where T: Foo {";
    assert_eq!(impl_self_type_offset(line), line.find("Vec"));
    assert_eq!(impl_self_type_offset("fn impl_thing() {}"), None);
  }

  #[test]
  fn should_only_index_names_made_of_word_characters() {
    assert!(is_word_like("get_order"));
    assert!(is_word_like("$scope"));
    assert!(!is_word_like("#secret"));
    assert!(!is_word_like(""));
  }
}
