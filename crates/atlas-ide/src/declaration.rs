use atlas_lsp::SyncDemand;
use atlas_lsp::protocol::{GotoResponse, Location};
use atlas_query::{Response, SymbolTarget, Targeted};
use regex::RegexBuilder;
use serde::Serialize;

use crate::error::{IdeError, IdeResult};
use crate::map::{Site, SiteMapper, SitePlace};
use crate::references::{TargetView, anchor_place, position_params, target_view, unsupported};
use crate::render::ServerView;
use crate::service::{IdeService, Session};
use crate::target::{Anchor, FileRef, Resolver, Target, read_text};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclarationRequest {
  Position { target: String },
  Regex { path: String, regex: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExactDeclaration {
  pub at: SitePlace,
  pub server: ServerView,
  pub definitions: Vec<Site>,
  pub declarations: Vec<Site>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExactImplementations {
  pub target: TargetView,
  pub server: ServerView,
  pub sites: Vec<Site>,
}

impl IdeService {
  pub fn declaration(&self, request: &DeclarationRequest) -> IdeResult<Response<ExactDeclaration>> {
    self.ensure_enabled()?;
    let files = self.files();
    let freshness = self.query.fresh()?;
    let anchor = match request {
      DeclarationRequest::Position { target } => match SymbolTarget::parse(target)? {
        SymbolTarget::Position { repo, path, line, column } => {
          let file = files.locate(repo.as_deref(), &path)?;
          Resolver {
            query: &self.query,
            files: &files,
          }
          .anchor(&Target::Position { file, line, column })?
        }
        _ => {
          return Err(IdeError::InvalidRequest(format!(
            "declaration takes [repo:]path:line:col or a path with a regex; {target:?} is not a position"
          )));
        }
      },
      DeclarationRequest::Regex { path, regex } => regex_anchor(files.locate_spec(path)?, regex)?,
    };
    let (server, (definitions, declarations)) = self.exact(&freshness, &files, &anchor.file, SyncDemand::Best, std::slice::from_ref(&anchor.file.absolute), |session| {
      let capabilities = session.capabilities();
      let (definition, declaration) = (capabilities.definition_provider.is_supported(), capabilities.declaration_provider.is_supported());
      if !definition && !declaration {
        return Err(unsupported(session, "textDocument/definition"));
      }
      let mut mapper = SiteMapper::new(session);
      let definitions = match definition {
        true => mapper.map_all(goto(session, "textDocument/definition", &anchor)?)?,
        false => Vec::new(),
      };
      let declarations = match declaration {
        true => mapper.map_all(goto(session, "textDocument/declaration", &anchor)?)?,
        false => Vec::new(),
      };
      let declarations = declarations
        .into_iter()
        .filter(|site| !definitions.iter().any(|definition| definition.place == site.place))
        .collect();
      Ok((definitions, declarations))
    })?;
    Ok(Response {
      freshness,
      answer: ExactDeclaration {
        at: anchor_place(&anchor),
        server,
        definitions,
        declarations,
      },
    })
  }

  pub fn implementations(&self, target: &str) -> IdeResult<Response<Targeted<ExactImplementations>>> {
    self.ensure_enabled()?;
    let files = self.files();
    let mut freshness = self.query.fresh()?;
    let resolver = Resolver {
      query: &self.query,
      files: &files,
    };
    let target = match resolver.resolve(target, &mut freshness)? {
      Targeted::Found(target) => target,
      Targeted::NotFound { spec, hint } => {
        return Ok(Response {
          freshness,
          answer: Targeted::NotFound { spec, hint },
        });
      }
      Targeted::Ambiguous { spec, candidates } => {
        return Ok(Response {
          freshness,
          answer: Targeted::Ambiguous { spec, candidates },
        });
      }
    };
    let anchor = resolver.anchor(&target)?;
    let (server, sites) = self.exact(&freshness, &files, &anchor.file, SyncDemand::Best, std::slice::from_ref(&anchor.file.absolute), |session| {
      if !session.capabilities().implementation_provider.is_supported() {
        return Err(unsupported(session, "textDocument/implementation"));
      }
      SiteMapper::new(session).map_all(goto(session, "textDocument/implementation", &anchor)?)
    })?;
    Ok(Response {
      freshness,
      answer: Targeted::Found(ExactImplementations {
        target: target_view(&target, &anchor),
        server,
        sites,
      }),
    })
  }
}

fn goto(session: &Session<'_>, method: &str, anchor: &Anchor) -> IdeResult<Vec<Location>> {
  let response: GotoResponse = session.request(method, position_params(session, anchor)?)?;
  Ok(response.into_locations())
}

pub(crate) fn regex_anchor(file: FileRef, pattern: &str) -> IdeResult<Anchor> {
  let regex = RegexBuilder::new(pattern)
    .multi_line(true)
    .dot_matches_new_line(true)
    .build()
    .map_err(|error| IdeError::Regex {
      pattern: pattern.to_owned(),
      reason: error.to_string(),
    })?;
  let groups = regex.captures_len() - 1;
  if groups != 1 {
    return Err(IdeError::RegexGroups {
      pattern: pattern.to_owned(),
      groups,
    });
  }
  let (text, hash) = read_text(&file.absolute).map_err(|reason| IdeError::InvalidRequest(format!("cannot search {file}: {reason}")))?;
  let matches: Vec<regex::Captures<'_>> = regex.captures_iter(&text).collect();
  let [only] = matches.as_slice() else {
    return Err(IdeError::RegexMatches {
      pattern: pattern.to_owned(),
      file: file.to_string(),
      count: matches.len(),
    });
  };
  let Some(group) = only.get(1) else {
    return Err(IdeError::InvalidRequest(format!(
      "the capture group of {pattern:?} did not take part in the match in {file}; make it mandatory"
    )));
  };
  let offset = group.start();
  Ok(Anchor { file, text, hash, offset })
}

#[cfg(test)]
mod tests {
  use std::fs;

  use atlas_core::{RelPath, RepoName};

  use super::*;

  fn file(text: &str) -> (tempfile::TempDir, FileRef) {
    let dir = tempfile::tempdir().unwrap();
    let absolute = dir.path().join("a.ts");
    fs::write(&absolute, text).unwrap();
    let file = FileRef {
      repo: RepoName::new("web").unwrap(),
      path: RelPath::new("a.ts").unwrap(),
      absolute,
    };
    (dir, file)
  }

  #[test]
  fn should_anchor_a_regex_at_its_only_capture_group() {
    let (_dir, file) = file("import { x } from './x';\nconst y = x(1);\n");

    let anchor = regex_anchor(file, r"const y = (x)\(").unwrap();

    assert_eq!(anchor.offset, 35);
    assert_eq!(
      anchor_place(&anchor),
      SitePlace {
        repo: Some("web".to_owned()),
        path: "a.ts".to_owned(),
        line: 2,
        column: Some(11)
      }
    );
  }

  #[test]
  fn should_refuse_a_regex_without_exactly_one_group_or_one_match() {
    let (_dir, file) = file("x(1);\nx(2);\n");

    let none = regex_anchor(file.clone(), r"x\(").unwrap_err().to_string();
    let two = regex_anchor(file.clone(), r"(x)\((\d)").unwrap_err().to_string();
    let many = regex_anchor(file.clone(), r"(x)\(").unwrap_err().to_string();
    let zero = regex_anchor(file.clone(), r"(y)\(").unwrap_err().to_string();
    let invalid = regex_anchor(file, r"(x").unwrap_err();

    assert_eq!(none, "the regex \"x\\\\(\" has 0 capture groups; it needs exactly one, around the name to look up");
    assert_eq!(two, "the regex \"(x)\\\\((\\\\d)\" has 2 capture groups; it needs exactly one, around the name to look up");
    assert_eq!(many, "the regex \"(x)\\\\(\" matches 2 times in web:a.ts; it must match exactly once");
    assert_eq!(zero, "the regex \"(y)\\\\(\" matches 0 times in web:a.ts; it must match exactly once");
    assert!(matches!(invalid, IdeError::Regex { .. }), "{invalid}");
  }
}
