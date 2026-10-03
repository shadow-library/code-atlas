use atlas_core::{Attrs, EdgeKind, EntityKey, FileFacts, LanguageFamily, ModuleBody, NodeRef, RelPath, SymbolFact};
use serde_json::json;

use super::detector;
use crate::cert::Cert;
use crate::sink::{Sink, attrs};
use crate::view::FileView;

pub struct TestDetector;

detector!(TestDetector, "tests", 1);

const TS_TEST_MARKERS: [&str; 2] = [".spec.", ".test."];

impl TestDetector {
  fn prefilter(&self, facts: &FileFacts) -> bool {
    facts.symbols.iter().any(|symbol| symbol.test.is_some())
  }

  fn run(&self, view: &FileView<'_>, sink: &mut Sink) {
    if view.family() == LanguageFamily::Rust {
      self.inline_test_modules(view, sink);
    }
    let Some(subject) = subject_file(view.family(), view.facts.file.path.as_str()) else {
      return;
    };
    let Ok(subject) = RelPath::new(&subject) else { return };
    let file = EntityKey::File {
      repo: view.repo().clone(),
      path: subject,
    };
    for symbol in view.facts.symbols.iter().filter(|symbol| is_top_level_test(view, symbol)) {
      sink.edge(
        EdgeKind::CoversFile,
        NodeRef::Local(symbol.local),
        NodeRef::Key(file.clone()),
        symbol.name_span,
        Cert::heuristic(0.6, "test-file-naming"),
        attrs([("convention", json!(convention(view.family(), view.facts.file.path.as_str())))]),
      );
    }
  }

  fn inline_test_modules(&self, view: &FileView<'_>, sink: &mut Sink) {
    for declaration in &view.facts.module_decls {
      let ModuleBody::Inline(local) = declaration.body else { continue };
      let is_cfg_test = declaration
        .attributes
        .iter()
        .any(|attribute| attribute.is("cfg") && attribute.args.iter().any(|arg| *arg == atlas_core::ArgSummary::Path(vec!["test".to_owned()])));
      if !is_cfg_test {
        continue;
      }
      sink.edge(
        EdgeKind::CoversFile,
        NodeRef::Local(local),
        NodeRef::Key(view.file_key()),
        declaration.span,
        Cert::Syntactic,
        Attrs::new(),
      );
    }
  }
}

fn is_top_level_test(view: &FileView<'_>, symbol: &SymbolFact) -> bool {
  symbol.test.is_some() && !symbol.parent.and_then(|parent| view.symbol(parent)).is_some_and(|parent| parent.test.is_some())
}

fn convention(family: LanguageFamily, path: &str) -> &'static str {
  match family {
    LanguageFamily::Rust if path.contains("tests/") && !path.ends_with("_test.rs") => "integration_tests_dir",
    LanguageFamily::Rust => "suffix_test",
    _ => "sibling_spec",
  }
}

pub(crate) fn subject_file(family: LanguageFamily, path: &str) -> Option<String> {
  match family {
    LanguageFamily::Rust => {
      if let Some(stem) = path.strip_suffix("_test.rs") {
        return Some(format!("{stem}.rs"));
      }
      let (root, rest) = match path.rsplit_once("/tests/") {
        Some((root, rest)) => (format!("{root}/"), rest),
        None => (String::new(), path.strip_prefix("tests/")?),
      };
      (!rest.is_empty()).then(|| format!("{root}src/lib.rs"))
    }
    _ => {
      let (directory, file) = path.rsplit_once('/').map_or(("", path), |(directory, file)| (directory, file));
      let marker = TS_TEST_MARKERS.iter().find_map(|marker| file.find(marker).map(|index| (index, marker.len())))?;
      let (stem, extension) = (&file[..marker.0], &file[marker.0 + marker.1..]);
      if stem.is_empty() || extension.contains('.') {
        return None;
      }
      let sibling = format!("{stem}.{extension}");
      Some(if directory.is_empty() { sibling } else { format!("{directory}/{sibling}") })
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_map_test_files_to_their_subjects() {
    assert_eq!(
      subject_file(LanguageFamily::Ts, "src/users/users.service.spec.ts").as_deref(),
      Some("src/users/users.service.ts")
    );
    assert_eq!(subject_file(LanguageFamily::Ts, "src/money.test.ts").as_deref(), Some("src/money.ts"));
    assert_eq!(subject_file(LanguageFamily::Ts, "Button.test.tsx").as_deref(), Some("Button.tsx"));
    assert_eq!(subject_file(LanguageFamily::Ts, "src/money.ts"), None);
    assert_eq!(subject_file(LanguageFamily::Rust, "src/foo_test.rs").as_deref(), Some("src/foo.rs"));
    assert_eq!(subject_file(LanguageFamily::Rust, "tests/invoices.rs").as_deref(), Some("src/lib.rs"));
    assert_eq!(subject_file(LanguageFamily::Rust, "crates/x/tests/a.rs").as_deref(), Some("crates/x/src/lib.rs"));
    assert_eq!(subject_file(LanguageFamily::Rust, "src/lib.rs"), None);
  }
}
