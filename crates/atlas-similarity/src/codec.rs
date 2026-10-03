use std::collections::{BTreeSet, HashMap, HashSet};

use atlas_core::{EntityId, EntityKind, RelPath, RepoName};

use crate::config::SimilarityConfig;
use crate::error::SimilarityError;
use crate::hashing::fnv1a;
use crate::index::{DocMeta, SimilarityIndex};
use crate::lexical::{FIELD_COUNT, LexicalDoc};
use crate::structural::{StructuralDoc, StructuralState};

const MAGIC: &[u8; 8] = b"ATLSIMIX";
const FORMAT_VERSION: u32 = 1;
const CHECKSUM_LEN: usize = 8;
const TAG_ABSENT: u8 = 0;
const TAG_TOO_SMALL: u8 = 1;
const TAG_INDEXED: u8 = 2;

fn corrupt(reason: impl Into<String>) -> SimilarityError {
  SimilarityError::Corrupt { reason: reason.into() }
}

struct Writer(Vec<u8>);

impl Writer {
  fn u8(&mut self, value: u8) {
    self.0.push(value);
  }

  fn u16(&mut self, value: u16) {
    self.0.extend_from_slice(&value.to_le_bytes());
  }

  fn u32(&mut self, value: u32) {
    self.0.extend_from_slice(&value.to_le_bytes());
  }

  fn u64(&mut self, value: u64) {
    self.0.extend_from_slice(&value.to_le_bytes());
  }

  fn len(&mut self, value: usize) {
    self.u64(value as u64);
  }

  fn str(&mut self, value: &str) {
    self.len(value.len());
    self.0.extend_from_slice(value.as_bytes());
  }
}

struct Reader<'a> {
  data: &'a [u8],
  position: usize,
}

impl<'a> Reader<'a> {
  fn take(&mut self, count: usize) -> Result<&'a [u8], SimilarityError> {
    let end = self.position.checked_add(count).ok_or(SimilarityError::Truncated)?;
    let slice = self.data.get(self.position..end).ok_or(SimilarityError::Truncated)?;
    self.position = end;
    Ok(slice)
  }

  fn array<const N: usize>(&mut self) -> Result<[u8; N], SimilarityError> {
    let mut out = [0u8; N];
    out.copy_from_slice(self.take(N)?);
    Ok(out)
  }

  fn u8(&mut self) -> Result<u8, SimilarityError> {
    Ok(self.array::<1>()?[0])
  }

  fn u16(&mut self) -> Result<u16, SimilarityError> {
    Ok(u16::from_le_bytes(self.array()?))
  }

  fn u32(&mut self) -> Result<u32, SimilarityError> {
    Ok(u32::from_le_bytes(self.array()?))
  }

  fn u64(&mut self) -> Result<u64, SimilarityError> {
    Ok(u64::from_le_bytes(self.array()?))
  }

  fn len(&mut self) -> Result<usize, SimilarityError> {
    usize::try_from(self.u64()?).map_err(|_| corrupt("length does not fit in memory"))
  }

  fn count(&mut self, min_element_bytes: usize) -> Result<usize, SimilarityError> {
    let count = self.len()?;
    let remaining = self.data.len() - self.position;
    if count.checked_mul(min_element_bytes).is_none_or(|needed| needed > remaining) {
      return Err(SimilarityError::Truncated);
    }
    Ok(count)
  }

  fn str(&mut self) -> Result<&'a str, SimilarityError> {
    let len = self.len()?;
    std::str::from_utf8(self.take(len)?).map_err(|_| corrupt("string is not valid utf-8"))
  }
}

impl SimilarityIndex {
  pub fn to_bytes(&self) -> Vec<u8> {
    let slots: Vec<u32> = (0..self.metas.len() as u32).filter(|&slot| self.metas[slot as usize].is_some()).collect();
    let used_terms: BTreeSet<u32> = slots
      .iter()
      .filter_map(|&slot| self.lexical.doc(slot))
      .flat_map(|doc| doc.terms.iter().map(|&(term, _)| term))
      .collect();
    let remap: HashMap<u32, u32> = used_terms.iter().enumerate().map(|(dense, &term)| (term, dense as u32)).collect();

    let mut out = Writer(Vec::new());
    out.0.extend_from_slice(MAGIC);
    out.u32(FORMAT_VERSION);
    out.len(self.config.bands);
    out.len(self.config.rows);
    out.len(self.config.min_tokens);
    out.len(used_terms.len());
    for &term in &used_terms {
      out.str(self.lexical.term(term).unwrap_or(""));
    }
    out.len(slots.len());
    for &slot in &slots {
      let Some(meta) = self.metas[slot as usize].as_ref() else { continue };
      out.u64(meta.entity.raw());
      out.str(meta.kind.as_str());
      out.str(meta.repo.as_str());
      out.str(&meta.name);
      out.str(meta.path.as_str());
      let (lens, terms) = self.lexical.doc(slot).map_or(([0; FIELD_COUNT], &[][..]), |doc| (doc.lens, doc.terms.as_slice()));
      for len in lens {
        out.u32(len);
      }
      out.len(terms.len());
      for &(term, tf) in terms {
        out.u32(remap[&term]);
        for count in tf {
          out.u16(count);
        }
      }
      match self.structural.state(slot) {
        StructuralState::Absent => out.u8(TAG_ABSENT),
        StructuralState::TooSmall(tokens) => {
          out.u8(TAG_TOO_SMALL);
          out.len(*tokens);
        }
        StructuralState::Indexed(doc) => {
          out.u8(TAG_INDEXED);
          out.len(doc.tokens);
          out.len(doc.shingles.len());
          for &shingle in &doc.shingles {
            out.u64(shingle);
          }
        }
      }
    }
    let checksum = fnv1a(&out.0);
    out.u64(checksum);
    out.0
  }

  pub fn from_bytes(bytes: &[u8]) -> Result<Self, SimilarityError> {
    if bytes.len() < MAGIC.len() || &bytes[..MAGIC.len()] != MAGIC {
      return Err(SimilarityError::BadMagic);
    }
    let mut header = Reader {
      data: bytes,
      position: MAGIC.len(),
    };
    let found = header.u32()?;
    if found != FORMAT_VERSION {
      return Err(SimilarityError::UnsupportedVersion { found, expected: FORMAT_VERSION });
    }
    if bytes.len() < header.position + CHECKSUM_LEN {
      return Err(SimilarityError::Truncated);
    }
    let (payload, trailer) = bytes.split_at(bytes.len() - CHECKSUM_LEN);
    let mut stored = [0u8; CHECKSUM_LEN];
    stored.copy_from_slice(trailer);
    if fnv1a(payload) != u64::from_le_bytes(stored) {
      return Err(SimilarityError::ChecksumMismatch);
    }
    let mut reader = Reader {
      data: payload,
      position: header.position,
    };
    let config = SimilarityConfig {
      bands: reader.len()?,
      rows: reader.len()?,
      min_tokens: reader.len()?,
    };
    let mut index = SimilarityIndex::new(config).map_err(|error| corrupt(error.to_string()))?;

    let term_count = reader.count(8)?;
    let mut seen_terms: HashSet<&str> = HashSet::with_capacity(term_count);
    for _ in 0..term_count {
      let term = reader.str()?;
      if !seen_terms.insert(term) {
        return Err(corrupt("duplicate term in term table"));
      }
      index.lexical.intern(term);
    }

    let doc_count = reader.count(8)?;
    for _ in 0..doc_count {
      let entity = EntityId::from_raw(reader.u64()?);
      if index.contains(entity) {
        return Err(corrupt("duplicate entity"));
      }
      let kind: EntityKind = reader.str()?.parse().map_err(|error: atlas_core::CoreError| corrupt(error.to_string()))?;
      let repo = RepoName::new(reader.str()?).map_err(|error| corrupt(error.to_string()))?;
      let name = reader.str()?.to_owned();
      let path = RelPath::new(reader.str()?).map_err(|error| corrupt(error.to_string()))?;

      let mut lens = [0u32; FIELD_COUNT];
      for len in &mut lens {
        *len = reader.u32()?;
      }
      let tf_count = reader.count(4 + 2 * FIELD_COUNT)?;
      let mut terms: Vec<(u32, [u16; FIELD_COUNT])> = Vec::with_capacity(tf_count);
      for _ in 0..tf_count {
        let term = reader.u32()?;
        if term as usize >= term_count || terms.last().is_some_and(|&(previous, _)| previous >= term) {
          return Err(corrupt("term ids must be in range and strictly increasing"));
        }
        let mut tf = [0u16; FIELD_COUNT];
        for count in &mut tf {
          *count = reader.u16()?;
        }
        if tf.iter().zip(lens).any(|(&count, len)| count > 0 && len == 0) {
          return Err(corrupt("term frequency in a field of length zero"));
        }
        terms.push((term, tf));
      }
      let state = match reader.u8()? {
        TAG_ABSENT => StructuralState::Absent,
        TAG_TOO_SMALL => StructuralState::TooSmall(reader.len()?),
        TAG_INDEXED => {
          let tokens = reader.len()?;
          let shingle_count = reader.count(8)?;
          let mut shingles = Vec::with_capacity(shingle_count);
          for _ in 0..shingle_count {
            let shingle = reader.u64()?;
            if shingles.last().is_some_and(|&previous| previous >= shingle) {
              return Err(corrupt("shingles must be strictly increasing"));
            }
            shingles.push(shingle);
          }
          StructuralState::Indexed(StructuralDoc::from_shingles(tokens, shingles))
        }
        other => return Err(corrupt(format!("unknown structural tag {other}"))),
      };
      let slot = index.allocate(DocMeta { entity, kind, repo, name, path })?;
      index.structural.set(slot, state);
      index.lexical.insert(slot, LexicalDoc { lens, terms });
    }
    if reader.position != payload.len() {
      return Err(corrupt("trailing bytes after the last document"));
    }
    Ok(index)
  }
}
