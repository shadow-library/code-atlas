use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, PoisonError};
use std::time::Instant;

use atlas_core::{RelPath, RepoName};
use atlas_project::{DiscoveredDoc, DocScope, Project, RepoConfig, discover_docs};

use super::markdown::{ParsedDoc, parse};
use super::rank::SectionTerms;
use crate::QueryEngine;
use crate::model::Caveat;

const MAX_DOCS: usize = 2_000;

type Loaded = (Arc<ParsedDoc>, Arc<Vec<SectionTerms>>);

#[derive(Default)]
pub(crate) struct DocCache {
  entries: HashMap<PathBuf, Cached>,
  listings: HashMap<DocScope, Listing>,
}

struct Cached {
  mtime_ns: i64,
  size: u64,
  parsed: Arc<ParsedDoc>,
  terms: Arc<Vec<SectionTerms>>,
}

#[derive(Clone)]
struct Listed {
  repo: RepoName,
  root: PathBuf,
  doc: DiscoveredDoc,
}

struct Listing {
  repos: Vec<RepoConfig>,
  events: u64,
  walked: Instant,
  found: Vec<Listed>,
  dropped: usize,
}

pub(crate) struct LoadedDoc {
  pub repo: RepoName,
  pub path: RelPath,
  pub absolute: PathBuf,
  pub note: bool,
  pub parsed: Arc<ParsedDoc>,
  pub terms: Arc<Vec<SectionTerms>>,
}

impl LoadedDoc {
  pub fn title(&self) -> String {
    let file = self.path.as_str().rsplit('/').next().unwrap_or(self.path.as_str());
    self
      .parsed
      .sections
      .iter()
      .find(|section| !section.preamble)
      .map_or_else(|| file.to_owned(), |section| section.title().to_owned())
  }
}

pub(crate) struct Corpus {
  pub docs: Vec<LoadedDoc>,
  pub dropped: usize,
}

impl Corpus {
  pub fn empty() -> Self {
    Self { docs: Vec::new(), dropped: 0 }
  }

  pub fn caveats(&self) -> Vec<Caveat> {
    match self.dropped {
      0 => Vec::new(),
      dropped => vec![Caveat::Note {
        message: format!("{dropped} more docs were not read: at most {MAX_DOCS} docs besides the notes are read per project"),
      }],
    }
  }
}

fn walk(project: &Project, scope: DocScope) -> (Vec<Listed>, usize) {
  let mut notes = Vec::new();
  let mut docs = Vec::new();
  for repo in &project.repos {
    match discover_docs(repo, scope, project.settings.max_file_bytes) {
      Ok(found) => {
        for doc in found {
          let listed = Listed {
            repo: repo.name.clone(),
            root: repo.path.clone(),
            doc,
          };
          match listed.doc.note {
            true => notes.push(listed),
            false => docs.push(listed),
          }
        }
      }
      Err(error) => tracing::warn!(repo = %repo.name, error = %error, "cannot list the docs of a repo"),
    }
  }
  let dropped = docs.len().saturating_sub(MAX_DOCS);
  docs.truncate(MAX_DOCS);
  notes.extend(docs);
  (notes, dropped)
}

fn load(absolute: &PathBuf, path: &RelPath) -> Option<Loaded> {
  let text = String::from_utf8(fs::read(absolute).ok()?).ok()?;
  let name = path.as_str().rsplit('/').next().unwrap_or(path.as_str());
  let parsed = parse(&text, name);
  let terms = parsed.sections.iter().map(SectionTerms::of).collect();
  Some((Arc::new(parsed), Arc::new(terms)))
}

impl QueryEngine {
  pub(crate) fn corpus(&self, scope: DocScope) -> Corpus {
    let project = self.engine().project();
    let watched = self.engine().healthy_watch_events();
    let interval = self.engine().full_scan_interval();
    let reused = {
      let cache = self.doc_cache().lock().unwrap_or_else(PoisonError::into_inner);
      cache
        .listings
        .get(&scope)
        .filter(|listing| watched == Some(listing.events) && listing.walked.elapsed() < interval && listing.repos == project.repos)
        .map(|listing| (listing.found.clone(), listing.dropped))
    };
    let (found, dropped) = reused.unwrap_or_else(|| {
      let walked = Instant::now();
      let (found, dropped) = walk(&project, scope);
      if let Some(events) = watched {
        let listing = Listing {
          repos: project.repos.clone(),
          events,
          walked,
          found: found.clone(),
          dropped,
        };
        self.doc_cache().lock().unwrap_or_else(PoisonError::into_inner).listings.insert(scope, listing);
      }
      (found, dropped)
    });
    let mut slots: Vec<Option<Loaded>> = {
      let cache = self.doc_cache().lock().unwrap_or_else(PoisonError::into_inner);
      found
        .iter()
        .map(|listed| {
          let cached = cache.entries.get(&listed.root.join(listed.doc.path.as_str()))?;
          (cached.mtime_ns == listed.doc.mtime_ns && cached.size == listed.doc.size).then(|| (Arc::clone(&cached.parsed), Arc::clone(&cached.terms)))
        })
        .collect()
    };
    let mut loaded = Vec::new();
    for (slot, listed) in slots.iter_mut().zip(&found).filter(|(slot, _)| slot.is_none()) {
      let absolute = listed.root.join(listed.doc.path.as_str());
      *slot = load(&absolute, &listed.doc.path);
      if let Some((parsed, terms)) = slot {
        loaded.push((absolute, listed.doc.mtime_ns, listed.doc.size, Arc::clone(parsed), Arc::clone(terms)));
      }
    }
    {
      let mut cache = self.doc_cache().lock().unwrap_or_else(PoisonError::into_inner);
      for (absolute, mtime_ns, size, parsed, terms) in loaded {
        cache.entries.insert(absolute, Cached { mtime_ns, size, parsed, terms });
      }
      if scope == DocScope::All {
        let seen: HashSet<PathBuf> = found.iter().map(|listed| listed.root.join(listed.doc.path.as_str())).collect();
        cache.entries.retain(|path, _| seen.contains(path));
      }
    }
    let docs = found
      .into_iter()
      .zip(slots)
      .filter_map(|(listed, slot)| {
        let (parsed, terms) = slot?;
        Some(LoadedDoc {
          absolute: listed.root.join(listed.doc.path.as_str()),
          repo: listed.repo,
          path: listed.doc.path,
          note: listed.doc.note,
          parsed,
          terms,
        })
      })
      .collect();
    Corpus { docs, dropped }
  }
}
