use std::collections::BTreeMap;

use atlas_core::{ContentHash, Entity, RelPath, RepoName};
use atlas_semantic::{ClientCall, ClientCallLink, EndpointCandidate, SemanticTask, SimilarityPair, SourceRef, SymbolSnippet};

use super::links::LinkCandidate;
use crate::engine::Engine;

#[derive(Default)]
pub(super) struct Sources {
  files: BTreeMap<(RepoName, RelPath), Option<(String, ContentHash)>>,
}

impl Sources {
  pub(super) fn get(&mut self, engine: &Engine, repo: &RepoName, path: &RelPath) -> Option<(&str, ContentHash)> {
    let loaded = self.files.entry((repo.clone(), path.clone())).or_insert_with(|| match engine.read_source(repo, path) {
      Ok(source) => Some(source),
      Err(error) => {
        tracing::debug!(repo = %repo, path = %path, error = %error, "source unavailable for a semantic task");
        None
      }
    });
    loaded.as_ref().map(|(text, hash)| (text.as_str(), *hash))
  }

  fn located(&mut self, engine: &Engine, entity: &Entity) -> Option<(String, SourceRef)> {
    let (repo, path, span) = (entity.repo.clone()?, entity.path.clone()?, entity.span?);
    let (text, file_hash) = self.get(engine, &repo, &path)?;
    let snippet = span.text(text)?.to_owned();
    Some((snippet, SourceRef { repo, path, span, file_hash }))
  }

  pub(super) fn snippet(&mut self, engine: &Engine, entity: &Entity) -> Option<SymbolSnippet> {
    let (snippet, source) = self.located(engine, entity)?;
    Some(SymbolSnippet {
      entity: entity.id,
      key: entity.key.to_string(),
      signature: entity.signature.clone(),
      snippet,
      source,
      callers: None,
      test_names: Vec::new(),
    })
  }

  pub(super) fn pair_task(&mut self, engine: &Engine, left: &Entity, right: &Entity) -> Option<SemanticTask> {
    let pair = SimilarityPair::new(self.snippet(engine, left)?, self.snippet(engine, right)?).ok()?;
    Some(SemanticTask::ClassifySimilarityPair(pair))
  }

  pub(super) fn link_task(&mut self, engine: &Engine, candidate: &LinkCandidate) -> Option<SemanticTask> {
    let site = candidate.site.as_ref()?;
    let (snippet, source) = match self.located(engine, &site.caller) {
      Some(located) => located,
      None => {
        let evidence = &site.evidence;
        let (text, file_hash) = self.get(engine, &evidence.repo, &evidence.path)?;
        let snippet = evidence.span.text(text)?.to_owned();
        let source = SourceRef {
          repo: evidence.repo.clone(),
          path: evidence.path.clone(),
          span: evidence.span,
          file_hash,
        };
        (snippet, source)
      }
    };
    let call = ClientCall {
      call_site: candidate.call.id,
      method: Some(candidate.method.as_str().to_owned()),
      url_pattern: candidate.target.clone(),
      snippet,
      source,
    };
    let mut endpoints = Vec::new();
    for choice in &candidate.endpoints {
      let endpoint = &choice.endpoint;
      let (Some(repo), Some(path), Some(span)) = (endpoint.repo.clone(), endpoint.path.clone(), endpoint.span) else {
        continue;
      };
      let Some((_, file_hash)) = self.get(engine, &repo, &path) else { continue };
      endpoints.push(EndpointCandidate {
        entity: endpoint.id,
        method: choice.method.clone(),
        route: choice.route.clone(),
        handler_signature: choice.handler_signature.clone(),
        source: SourceRef { repo, path, span, file_hash },
      });
    }
    ClientCallLink::new(call, endpoints).ok().map(SemanticTask::LinkUnresolvedClientCall)
  }
}
