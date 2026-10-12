use std::collections::BTreeSet;

const MACOS_PRIVATE_PREFIX: &str = "/private";
const MACOS_PRIVATE_ALIASES: [&str; 2] = ["/var/", "/tmp/"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cwd {
  InTree(String),
  Outside,
}

impl Cwd {
  pub fn root() -> Self {
    Self::InTree(String::new())
  }

  pub fn in_tree(&self) -> Option<&str> {
    match self {
      Self::InTree(dir) => Some(dir),
      Self::Outside => None,
    }
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeIndex {
  roots: Vec<String>,
  files: BTreeSet<String>,
  dirs: BTreeSet<String>,
}

impl TreeIndex {
  pub fn new<R, F>(roots: R, files: F) -> Self
  where
    R: IntoIterator<Item = String>,
    F: IntoIterator<Item = String>,
  {
    let mut index = Self {
      roots: root_aliases(roots),
      ..Self::default()
    };
    for file in files {
      index.add_file(&file);
    }
    index
  }

  pub fn is_file(&self, path: &str) -> bool {
    self.files.contains(path)
  }

  pub fn is_dir(&self, path: &str) -> bool {
    path.is_empty() || self.dirs.contains(path)
  }

  pub fn add_file(&mut self, path: &str) {
    if path.is_empty() {
      return;
    }
    if let Some((parent, _)) = path.rsplit_once('/') {
      self.add_dir(parent);
    }
    self.files.insert(path.to_owned());
  }

  pub fn add_dir(&mut self, path: &str) {
    let mut end = 0;
    while let Some(offset) = path[end..].find('/') {
      end += offset;
      self.dirs.insert(path[..end].to_owned());
      end += 1;
    }
    if !path.is_empty() {
      self.dirs.insert(path.to_owned());
    }
  }

  pub fn absolute(&self, path: &str) -> String {
    let root = self.roots.first().map_or("", String::as_str);
    if path.is_empty() { root.to_owned() } else { format!("{root}/{path}") }
  }

  pub fn resolve(&self, cwd: &Cwd, raw: &str) -> Option<String> {
    if raw.is_empty() || raw.starts_with('~') {
      return None;
    }
    if !raw.starts_with('/') {
      return lexical(&format!("{}/{raw}", cwd.in_tree()?));
    }
    let absolute = format!("/{}", lexical_clamped(raw));
    self.roots.iter().find_map(|root| match absolute.strip_prefix(root.as_str()) {
      Some("") => Some(String::new()),
      Some(rest) => rest.strip_prefix('/').map(str::to_owned),
      None => None,
    })
  }

  pub fn glob(&self, cwd: &Cwd, pattern: &str) -> Vec<String> {
    let Some(anchored) = self.anchor_pattern(cwd, pattern) else {
      return Vec::new();
    };
    let tokens = glob_tokens(&anchored);
    self.files.iter().filter(|file| glob_matches(&tokens, &file.chars().collect::<Vec<_>>())).cloned().collect()
  }

  pub fn distinguishing_suffix(&self, path: &str) -> String {
    let components: Vec<&str> = path.split('/').collect();
    (1..=components.len())
      .map(|count| components[components.len() - count..].join("/"))
      .find(|suffix| {
        let nested = format!("/{suffix}");
        !self.files.iter().any(|file| file != path && (file == suffix || file.ends_with(&nested)))
      })
      .unwrap_or_else(|| path.to_owned())
  }

  fn anchor_pattern(&self, cwd: &Cwd, pattern: &str) -> Option<String> {
    let joined = if pattern.starts_with('/') {
      let root = self.roots.iter().find(|root| pattern.starts_with(&format!("{root}/")))?;
      pattern[root.len() + 1..].to_owned()
    } else {
      format!("{}/{pattern}", cwd.in_tree()?)
    };
    lexical(&joined)
  }
}

fn root_aliases(roots: impl IntoIterator<Item = String>) -> Vec<String> {
  let mut aliases = Vec::new();
  for root in roots {
    let root = format!("/{}", lexical_clamped(&root));
    let alias = match root.strip_prefix(MACOS_PRIVATE_PREFIX) {
      Some(rest) if MACOS_PRIVATE_ALIASES.iter().any(|prefix| rest.starts_with(prefix)) => Some(rest.to_owned()),
      None if MACOS_PRIVATE_ALIASES.iter().any(|prefix| root.starts_with(prefix)) => Some(format!("{MACOS_PRIVATE_PREFIX}{root}")),
      _ => None,
    };
    for candidate in std::iter::once(root).chain(alias) {
      if !aliases.contains(&candidate) {
        aliases.push(candidate);
      }
    }
  }
  aliases
}

fn lexical(path: &str) -> Option<String> {
  let mut parts: Vec<&str> = Vec::new();
  for part in path.split('/') {
    match part {
      "" | "." => {}
      ".." => {
        parts.pop()?;
      }
      other => parts.push(other),
    }
  }
  Some(parts.join("/"))
}

fn lexical_clamped(path: &str) -> String {
  let mut parts: Vec<&str> = Vec::new();
  for part in path.split('/') {
    match part {
      "" | "." => {}
      ".." => {
        parts.pop();
      }
      other => parts.push(other),
    }
  }
  parts.join("/")
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GlobToken {
  Literal(char),
  One,
  Run,
  Deep,
  DeepDirs,
  Class { negated: bool, ranges: Vec<(char, char)> },
}

fn glob_tokens(pattern: &str) -> Vec<GlobToken> {
  let chars: Vec<char> = pattern.chars().collect();
  let mut tokens = Vec::new();
  let mut index = 0;
  while index < chars.len() {
    match chars[index] {
      '*' if chars.get(index + 1) == Some(&'*') && chars.get(index + 2) == Some(&'/') => {
        tokens.push(GlobToken::DeepDirs);
        index += 3;
      }
      '*' if chars.get(index + 1) == Some(&'*') => {
        tokens.push(GlobToken::Deep);
        index += 2;
      }
      '*' => {
        tokens.push(GlobToken::Run);
        index += 1;
      }
      '?' => {
        tokens.push(GlobToken::One);
        index += 1;
      }
      '[' => match glob_class(&chars[index + 1..]) {
        Some((class, consumed)) => {
          tokens.push(class);
          index += consumed + 1;
        }
        None => {
          tokens.push(GlobToken::Literal('['));
          index += 1;
        }
      },
      other => {
        tokens.push(GlobToken::Literal(unprotect_char(other)));
        index += 1;
      }
    }
  }
  tokens
}

fn glob_class(rest: &[char]) -> Option<(GlobToken, usize)> {
  let negated = matches!(rest.first(), Some('!' | '^'));
  let start = usize::from(negated);
  let close = rest.iter().skip(start + 1).position(|c| *c == ']')? + start + 1;
  let body = &rest[start..close];
  let mut ranges = Vec::new();
  let mut index = 0;
  while index < body.len() {
    if index + 2 < body.len() && body[index + 1] == '-' {
      ranges.push((body[index], body[index + 2]));
      index += 3;
    } else {
      ranges.push((body[index], body[index]));
      index += 1;
    }
  }
  Some((GlobToken::Class { negated, ranges }, close + 1))
}

fn glob_matches(tokens: &[GlobToken], path: &[char]) -> bool {
  let Some((token, rest)) = tokens.split_first() else {
    return path.is_empty();
  };
  match token {
    GlobToken::Literal(expected) => path.first() == Some(expected) && glob_matches(rest, &path[1..]),
    GlobToken::One => path.first().is_some_and(|c| *c != '/') && glob_matches(rest, &path[1..]),
    GlobToken::Class { negated, ranges } => {
      path
        .first()
        .is_some_and(|c| *c != '/' && ranges.iter().any(|(low, high)| low <= c && c <= high) != *negated)
        && glob_matches(rest, &path[1..])
    }
    GlobToken::Run => (0..=path.iter().position(|c| *c == '/').unwrap_or(path.len())).any(|taken| glob_matches(rest, &path[taken..])),
    GlobToken::Deep => (0..=path.len()).any(|taken| glob_matches(rest, &path[taken..])),
    GlobToken::DeepDirs => (0..=path.len())
      .filter(|taken| *taken == 0 || path[taken - 1] == '/')
      .any(|taken| glob_matches(rest, &path[taken..])),
  }
}

pub(super) const QUOTED_SPECIALS: [char; 7] = ['$', '`', '*', '?', '[', '{', '~'];
const PROTECTED_BASE: u32 = 0xE000;

pub(super) fn protect_char(c: char) -> char {
  QUOTED_SPECIALS
    .iter()
    .position(|special| *special == c)
    .and_then(|index| char::from_u32(PROTECTED_BASE + index as u32))
    .unwrap_or(c)
}

pub(super) fn unprotect_char(c: char) -> char {
  (c as u32)
    .checked_sub(PROTECTED_BASE)
    .and_then(|index| QUOTED_SPECIALS.get(index as usize))
    .copied()
    .unwrap_or(c)
}

pub(super) fn unprotect(text: &str) -> String {
  text.chars().map(unprotect_char).collect()
}

#[cfg(test)]
mod tests {
  use super::*;

  fn tree() -> TreeIndex {
    TreeIndex::new(
      ["/var/folders/x/ae-1/tree".to_owned()],
      [
        "crates/a/src/lib.rs",
        "crates/a/src/detectors/express.rs",
        "crates/a/tests/common/mod.rs",
        "crates/b/tests/common/mod.rs",
        "README.md",
      ]
      .map(str::to_owned),
    )
  }

  fn at(dir: &str) -> Cwd {
    Cwd::InTree(dir.to_owned())
  }

  #[test]
  fn should_resolve_relative_paths_against_the_working_directory() {
    let tree = tree();
    assert_eq!(tree.resolve(&at("crates/a"), "src/lib.rs"), Some("crates/a/src/lib.rs".to_owned()));
    assert_eq!(
      tree.resolve(&at("crates/a/src"), "../tests/./common/mod.rs"),
      Some("crates/a/tests/common/mod.rs".to_owned())
    );
    assert_eq!(tree.resolve(&at("crates"), ".."), Some(String::new()));
    assert_eq!(tree.resolve(&at("crates"), "../.."), None);
    assert_eq!(tree.resolve(&Cwd::Outside, "src/lib.rs"), None);
    assert_eq!(tree.resolve(&Cwd::root(), "~/notes.md"), None);
  }

  #[test]
  fn should_resolve_absolute_paths_under_either_spelling_of_the_tree_root() {
    let tree = tree();
    assert_eq!(tree.resolve(&Cwd::Outside, "/var/folders/x/ae-1/tree/README.md"), Some("README.md".to_owned()));
    assert_eq!(
      tree.resolve(&Cwd::Outside, "/private/var/folders/x/ae-1/tree/crates/a/../a/src/lib.rs"),
      Some("crates/a/src/lib.rs".to_owned())
    );
    assert_eq!(tree.resolve(&Cwd::root(), "/private/var/folders/x/ae-1/tree"), Some(String::new()));
    assert_eq!(tree.resolve(&Cwd::root(), "/var/folders/x/ae-1/tree-other/README.md"), None);
    assert_eq!(tree.resolve(&Cwd::root(), "/etc/hosts"), None);
  }

  #[test]
  fn should_know_directories_of_listed_and_added_files() {
    let mut tree = tree();
    assert!(tree.is_dir(""));
    assert!(tree.is_dir("crates/a/src/detectors"));
    assert!(!tree.is_dir("crates/a/src/lib.rs"));
    assert!(!tree.is_dir("target"));
    tree.add_file("crates/c/src/new.rs");
    assert!(tree.is_file("crates/c/src/new.rs"));
    assert!(tree.is_dir("crates/c"));
    tree.add_dir("dist/out");
    assert!(tree.is_dir("dist"));
    assert!(tree.is_dir("dist/out"));
  }

  #[test]
  fn should_expand_globs_against_listed_files() {
    let tree = tree();
    assert_eq!(tree.glob(&at("crates/a"), "src/*.rs"), vec!["crates/a/src/lib.rs"]);
    assert_eq!(
      tree.glob(&Cwd::root(), "crates/*/tests/common/mod.rs"),
      vec!["crates/a/tests/common/mod.rs", "crates/b/tests/common/mod.rs"]
    );
    assert_eq!(tree.glob(&Cwd::root(), "crates/a/**/*.rs").len(), 3);
    assert_eq!(tree.glob(&Cwd::root(), "**/*.md"), vec!["README.md"]);
    assert_eq!(tree.glob(&at("crates/a/src"), "../tests/common/mo?.rs"), vec!["crates/a/tests/common/mod.rs"]);
    assert_eq!(tree.glob(&Cwd::root(), "crates/[b-z]/tests/common/*.rs"), vec!["crates/b/tests/common/mod.rs"]);
    assert_eq!(tree.glob(&Cwd::root(), "/private/var/folders/x/ae-1/tree/*.md"), vec!["README.md"]);
    assert_eq!(tree.glob(&Cwd::Outside, "*.md"), Vec::<String>::new());
  }

  #[test]
  fn should_find_the_shortest_suffix_that_names_one_file() {
    let tree = tree();
    assert_eq!(tree.distinguishing_suffix("crates/a/src/detectors/express.rs"), "express.rs");
    assert_eq!(tree.distinguishing_suffix("crates/a/tests/common/mod.rs"), "a/tests/common/mod.rs");
    assert_eq!(tree.distinguishing_suffix("crates/new/file.rs"), "file.rs");
  }

  #[test]
  fn should_round_trip_protected_characters() {
    let protected: String = "a$b*c".chars().map(protect_char).collect();
    assert_ne!(protected, "a$b*c");
    assert_eq!(unprotect(&protected), "a$b*c");
  }
}
