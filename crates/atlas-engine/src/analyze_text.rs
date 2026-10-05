use std::panic::{self, AssertUnwindSafe};

use atlas_core::{AnalysisInput, ContentHash, CoreError, FileFacts, FileRef, Language, RelPath, RepoName};

use crate::analyze::panic_message;
use crate::engine::Engine;
use crate::error::{EngineError, EngineResult};

const BUFFER_REPO: &str = "unsaved";

fn buffer_path(language: Language) -> Option<&'static str> {
  match language {
    Language::Rust => Some("buffer.rs"),
    Language::TypeScript => Some("buffer.ts"),
    Language::Tsx => Some("buffer.tsx"),
    Language::JavaScript => Some("buffer.js"),
    Language::Jsx => Some("buffer.jsx"),
    _ => None,
  }
}

impl Engine {
  pub fn analyze_text(&self, language: Language, text: &str) -> EngineResult<FileFacts> {
    let unsupported = || EngineError::Core(CoreError::UnsupportedPath(language.to_string()));
    let path = buffer_path(language).ok_or_else(unsupported)?;
    let analyzer = self.toolchain.analyzer(language).ok_or_else(unsupported)?;
    let file = FileRef {
      repo: RepoName::new(BUFFER_REPO)?,
      path: RelPath::new(path)?,
      language,
      content_hash: ContentHash::of(text.as_bytes()),
      is_declaration_file: false,
    };
    let producer = analyzer.producer();
    let limit = self.project().settings.max_file_bytes;
    if text.len() as u64 > limit {
      return Ok(FileFacts::failed(file, producer, format!("buffer of {} bytes exceeds max_file_bytes {limit}", text.len())));
    }
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
      let facts = analyzer.analyze(&AnalysisInput { file: &file, source: text }).map_err(|error| error.to_string())?;
      facts.check_invariants(text.len()).map_err(|error| error.to_string())?;
      Ok::<FileFacts, String>(facts)
    }));
    Ok(match outcome {
      Ok(Ok(facts)) => facts,
      Ok(Err(reason)) => FileFacts::failed(file, producer, reason),
      Err(payload) => {
        let reason = format!("analyzer panicked: {}", panic_message(payload.as_ref()));
        tracing::error!(language = %language, reason = %reason, "buffer analysis failed");
        FileFacts::failed(file, producer, reason)
      }
    })
  }
}
