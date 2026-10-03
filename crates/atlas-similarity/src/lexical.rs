use std::collections::{BTreeMap, HashMap};

use atlas_core::EntityId;
use serde::Serialize;

use crate::doc::SimilarityDoc;
use crate::synonyms::{self, Expansion};
use crate::text;

const K1: f64 = 1.2;
const B: f64 = 0.75;
const EXPANSION_WEIGHT: f64 = 0.4;
pub(crate) const FIELD_COUNT: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
  Name,
  Signature,
  Doc,
  Path,
  References,
}

impl Field {
  pub const ALL: [Field; FIELD_COUNT] = [Field::Name, Field::Signature, Field::Doc, Field::Path, Field::References];

  pub fn weight(self) -> f64 {
    match self {
      Self::Name => 3.0,
      Self::Signature => 1.5,
      Self::Doc => 1.2,
      Self::Path => 0.8,
      Self::References => 1.0,
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldMatch {
  pub field: Field,
  pub terms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExpansionMatch {
  pub term: String,
  pub from: Vec<String>,
}

pub(crate) struct LexicalDoc {
  pub lens: [u32; FIELD_COUNT],
  pub terms: Vec<(u32, [u16; FIELD_COUNT])>,
}

#[derive(Clone, Copy)]
struct Posting {
  slot: u32,
  tf: [u16; FIELD_COUNT],
}

pub(crate) struct LexicalQuery {
  pub direct: Vec<String>,
  pub expansions: Vec<Expansion>,
}

impl LexicalQuery {
  pub fn parse(text: &str) -> Self {
    let terms = text::analyze_query(text);
    let expansions = synonyms::expand(&terms.direct, &terms.all);
    Self { direct: terms.direct, expansions }
  }
}

pub(crate) struct Ranked {
  pub slot: u32,
  pub entity: EntityId,
  pub score: f64,
}

pub(crate) struct Explanation {
  pub direct_terms: Vec<String>,
  pub fields: Vec<FieldMatch>,
  pub via_expansion: Vec<ExpansionMatch>,
}

#[derive(Default)]
pub(crate) struct LexicalStore {
  terms: Vec<String>,
  term_ids: HashMap<String, u32>,
  postings: Vec<Vec<Posting>>,
  docs: Vec<Option<LexicalDoc>>,
  lens: Vec<[u32; FIELD_COUNT]>,
  totals: [u64; FIELD_COUNT],
  live: usize,
}

impl LexicalStore {
  pub fn intern(&mut self, term: &str) -> u32 {
    if let Some(&id) = self.term_ids.get(term) {
      return id;
    }
    let id = self.terms.len() as u32;
    self.terms.push(term.to_owned());
    self.term_ids.insert(term.to_owned(), id);
    self.postings.push(Vec::new());
    id
  }

  pub fn term(&self, id: u32) -> Option<&str> {
    self.terms.get(id as usize).map(String::as_str)
  }

  pub fn analyze(&mut self, doc: &SimilarityDoc) -> LexicalDoc {
    let mut lens = [0u32; FIELD_COUNT];
    let mut counts: BTreeMap<u32, [u16; FIELD_COUNT]> = BTreeMap::new();
    let mut add = |store: &mut Self, field: usize, text: &str| {
      for term in text::analyze(text) {
        let id = store.intern(&term);
        lens[field] += 1;
        let tf = &mut counts.entry(id).or_default()[field];
        *tf = tf.saturating_add(1);
      }
    };
    add(self, 0, &doc.name);
    add(self, 1, doc.signature.as_deref().unwrap_or(""));
    add(self, 2, doc.doc.as_deref().unwrap_or(""));
    add(self, 3, doc.path.as_str());
    for name in &doc.referenced_names {
      add(self, 4, name);
    }
    LexicalDoc {
      lens,
      terms: counts.into_iter().collect(),
    }
  }

  pub fn insert(&mut self, slot: u32, doc: LexicalDoc) {
    let index = slot as usize;
    if self.docs.len() <= index {
      self.docs.resize_with(index + 1, || None);
      self.lens.resize(index + 1, [0; FIELD_COUNT]);
    }
    for &(term, tf) in &doc.terms {
      let list = &mut self.postings[term as usize];
      let position = list.partition_point(|posting| posting.slot < slot);
      list.insert(position, Posting { slot, tf });
    }
    for (total, len) in self.totals.iter_mut().zip(doc.lens) {
      *total += u64::from(len);
    }
    self.lens[index] = doc.lens;
    self.docs[index] = Some(doc);
    self.live += 1;
  }

  pub fn remove(&mut self, slot: u32) {
    let Some(doc) = self.docs.get_mut(slot as usize).and_then(Option::take) else { return };
    for &(term, _) in &doc.terms {
      let list = &mut self.postings[term as usize];
      if let Ok(position) = list.binary_search_by_key(&slot, |posting| posting.slot) {
        list.remove(position);
      }
    }
    for (total, len) in self.totals.iter_mut().zip(doc.lens) {
      *total -= u64::from(len);
    }
    self.lens[slot as usize] = [0; FIELD_COUNT];
    self.live -= 1;
  }

  pub fn doc(&self, slot: u32) -> Option<&LexicalDoc> {
    self.docs.get(slot as usize).and_then(Option::as_ref)
  }

  pub fn search(&self, query: &LexicalQuery, limit: usize, resolve: impl Fn(u32) -> Option<EntityId>) -> Vec<Ranked> {
    if self.live == 0 || limit == 0 {
      return Vec::new();
    }
    let weighted = self.weighted_terms(query);
    if weighted.is_empty() {
      return Vec::new();
    }
    let live = self.live as f64;
    let averages = self.totals.map(|total| total as f64 / live);
    let mut scores = vec![0.0f64; self.lens.len()];
    let mut touched: Vec<u32> = Vec::new();
    for (id, weight) in weighted {
      let list = &self.postings[id as usize];
      if list.is_empty() {
        continue;
      }
      let df = list.len() as f64;
      let idf = (1.0 + (live - df + 0.5) / (df + 0.5)).ln();
      for posting in list {
        let lens = &self.lens[posting.slot as usize];
        let mut tf = 0.0;
        for (field, &count) in posting.tf.iter().enumerate() {
          if count == 0 {
            continue;
          }
          let relative_length = if averages[field] > 0.0 { f64::from(lens[field]) / averages[field] } else { 1.0 };
          let normaliser = 1.0 - B + B * relative_length;
          tf += Field::ALL[field].weight() * f64::from(count) / normaliser;
        }
        let contribution = weight * idf * tf / (K1 + tf);
        if !contribution.is_finite() {
          continue;
        }
        let slot = posting.slot as usize;
        if scores[slot] == 0.0 {
          touched.push(posting.slot);
        }
        scores[slot] += contribution;
      }
    }
    let mut ranked: Vec<Ranked> = touched
      .into_iter()
      .filter_map(|slot| {
        resolve(slot).map(|entity| Ranked {
          slot,
          entity,
          score: scores[slot as usize],
        })
      })
      .collect();
    let order = |a: &Ranked, b: &Ranked| b.score.total_cmp(&a.score).then(a.entity.cmp(&b.entity));
    if ranked.len() > limit {
      ranked.select_nth_unstable_by(limit, order);
      ranked.truncate(limit);
    }
    ranked.sort_unstable_by(order);
    ranked
  }

  fn weighted_terms(&self, query: &LexicalQuery) -> Vec<(u32, f64)> {
    let direct = query.direct.iter().filter_map(|term| self.term_ids.get(term).map(|&id| (id, 1.0)));
    let expanded = query
      .expansions
      .iter()
      .filter_map(|expansion| self.term_ids.get(&expansion.term).map(|&id| (id, EXPANSION_WEIGHT)));
    direct.chain(expanded).collect()
  }

  pub fn explain(&self, slot: u32, query: &LexicalQuery) -> Explanation {
    let Some(doc) = self.doc(slot) else {
      return Explanation {
        direct_terms: Vec::new(),
        fields: Vec::new(),
        via_expansion: Vec::new(),
      };
    };
    let frequencies = |term: &str| -> Option<[u16; FIELD_COUNT]> {
      let id = *self.term_ids.get(term)?;
      doc.terms.binary_search_by_key(&id, |&(candidate, _)| candidate).ok().map(|position| doc.terms[position].1)
    };
    let mut per_field: Vec<Vec<String>> = vec![Vec::new(); FIELD_COUNT];
    let mut direct_terms = Vec::new();
    for term in &query.direct {
      let Some(tf) = frequencies(term) else { continue };
      direct_terms.push(term.clone());
      for (field, _) in tf.iter().enumerate().filter(|(_, count)| **count > 0) {
        per_field[field].push(term.clone());
      }
    }
    let mut via_expansion = Vec::new();
    for expansion in &query.expansions {
      let Some(tf) = frequencies(&expansion.term) else { continue };
      via_expansion.push(ExpansionMatch {
        term: expansion.term.clone(),
        from: expansion.from.clone(),
      });
      for (field, _) in tf.iter().enumerate().filter(|(_, count)| **count > 0) {
        per_field[field].push(expansion.term.clone());
      }
    }
    let fields = Field::ALL
      .iter()
      .zip(per_field)
      .filter(|(_, terms)| !terms.is_empty())
      .map(|(&field, terms)| FieldMatch { field, terms })
      .collect();
    Explanation {
      direct_terms,
      fields,
      via_expansion,
    }
  }

  pub fn estimated_bytes(&self) -> usize {
    let postings: usize = self.postings.iter().map(|list| list.capacity() * size_of::<Posting>()).sum();
    let terms: usize = self.terms.iter().map(|term| term.len() * 2 + 2 * size_of::<String>() + size_of::<u32>()).sum();
    let docs: usize = self
      .docs
      .iter()
      .flatten()
      .map(|doc| doc.terms.capacity() * size_of::<(u32, [u16; FIELD_COUNT])>() + size_of::<LexicalDoc>())
      .sum();
    postings + terms + docs + self.lens.capacity() * size_of::<[u32; FIELD_COUNT]>()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_never_score_nan_for_inconsistent_field_lengths() {
    let mut store = LexicalStore::default();
    let id = store.intern("alpha");
    store.insert(
      0,
      LexicalDoc {
        lens: [0; FIELD_COUNT],
        terms: vec![(id, [1, 0, 0, 0, 0])],
      },
    );
    let query = LexicalQuery {
      direct: vec!["alpha".to_owned()],
      expansions: Vec::new(),
    };
    let ranked = store.search(&query, 5, |slot| Some(EntityId::from_raw(u64::from(slot) + 1)));
    assert_eq!(ranked.len(), 1);
    assert!(ranked[0].score.is_finite() && ranked[0].score > 0.0);
  }
}
