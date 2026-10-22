use atlas_core::{BodyFingerprintInput, EntityId, EntityKind, NormToken, RelPath, RepoName, token_code};

use crate::{SearchOptions, SimilarityConfig, SimilarityDoc, SimilarityError, SimilarityIndex, StructuralStatus};

const KEYWORDS: &[&str] = &["fn", "let", "mut", "for", "in", "if", "else", "return", "while", "match"];

fn lex(source: &str) -> Vec<u32> {
  let chars: Vec<char> = source.chars().collect();
  let mut tokens = Vec::new();
  let mut i = 0;
  while i < chars.len() {
    let c = chars[i];
    if c.is_whitespace() {
      i += 1;
    } else if c.is_alphabetic() || c == '_' {
      let start = i;
      while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
        i += 1;
      }
      let word: String = chars[start..i].iter().collect();
      tokens.push(if KEYWORDS.contains(&word.as_str()) {
        token_code(NormToken::Keyword(&word))
      } else {
        token_code(NormToken::Identifier)
      });
    } else if c.is_ascii_digit() {
      while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
      }
      tokens.push(token_code(NormToken::NumberLiteral));
    } else if c == '"' {
      i += 1;
      while i < chars.len() && chars[i] != '"' {
        i += 1;
      }
      i += 1;
      tokens.push(token_code(NormToken::StringLiteral));
    } else {
      tokens.push(token_code(NormToken::Punctuation(&c.to_string())));
      i += 1;
    }
  }
  tokens
}

fn body(source: &str) -> BodyFingerprintInput {
  let normalized_tokens = lex(source);
  BodyFingerprintInput {
    token_count: normalized_tokens.len() as u32,
    line_count: source.lines().count() as u32,
    normalized_tokens,
  }
}

fn entity(id: u64) -> EntityId {
  EntityId::from_raw(id)
}

fn doc(id: u64, name: &str, source: Option<&str>) -> SimilarityDoc {
  SimilarityDoc {
    entity: entity(id),
    kind: EntityKind::Function,
    repo: RepoName::new("repo").unwrap(),
    name: name.to_owned(),
    path: RelPath::new("src/lib.rs").unwrap(),
    signature: None,
    doc: None,
    referenced_names: Vec::new(),
    body: source.map(body),
  }
}

fn text_doc(id: u64, name: &str, doc_text: Option<&str>, path: &str) -> SimilarityDoc {
  SimilarityDoc {
    path: RelPath::new(path).unwrap(),
    doc: doc_text.map(str::to_owned),
    ..doc(id, name, None)
  }
}

const ORIGINAL: &str = r#"
fn process(items: Vec<Item>, limit: usize) -> Result<Total, Error> {
  let mut total = 0;
  for item in items.iter() {
    if item.price > limit { return Err(Error::TooLarge(item.id)); }
    total += item.price * item.qty;
  }
  let label = format!("total {}", total);
  log(label);
  while total > 100 { total -= 100; }
  match total { 0 => Ok(Total::Empty), n => Ok(Total::Value(n)) }
}
"#;

const RENAMED: &str = r#"
fn handle_batch(rows: Vec<Row>, cap: usize) -> Result<Sum, Failure> {
  let mut acc = 0;
  for row in rows.iter() {
    if row.cost > cap { return Err(Failure::Oversized(row.key)); }
    acc += row.cost * row.count;
  }
  let text = format!("sum {}", acc);
  emit(text);
  while acc > 100 { acc -= 100; }
  match acc { 0 => Ok(Sum::Nothing), m => Ok(Sum::Amount(m)) }
}
"#;

const REORDERED: &str = r#"
fn process(items: Vec<Item>, limit: usize) -> Result<Total, Error> {
  let mut total = 0;
  let label = format!("total {}", total);
  log(label);
  while total > 100 { total -= 100; }
  for item in items.iter() {
    if item.price > limit { return Err(Error::TooLarge(item.id)); }
    total += item.price * item.qty;
  }
  match total { 0 => Ok(Total::Empty), n => Ok(Total::Value(n)) }
}
"#;

const UNRELATED: &str = r#"
fn render(node: &Node, depth: usize, out: &mut String) {
  if node.children.is_empty() { out.push_str("leaf"); return; }
  let indent = "  ".repeat(depth);
  for child in node.children.iter() {
    out.push_str(&indent);
    out.push_str(child.label.as_str());
    out.push_str("\n");
    render(child, depth + 1, out);
  }
  if depth == 0 { out.push_str("end"); } else { out.push_str("up"); }
  while out.len() > 4096 { out.pop(); }
}
"#;

const TINY: &str = "fn f() { x }";

fn code_index() -> SimilarityIndex {
  let mut index = SimilarityIndex::default();
  for d in [
    doc(1, "process", Some(ORIGINAL)),
    doc(2, "handleBatch", Some(RENAMED)),
    doc(3, "processReordered", Some(REORDERED)),
    doc(4, "render", Some(UNRELATED)),
    doc(5, "id", Some(TINY)),
    doc(6, "noBody", None),
  ] {
    index.upsert(d).unwrap();
  }
  index
}

fn ids(matches: &[crate::StructuralMatch]) -> Vec<u64> {
  matches.iter().map(|m| m.entity.raw()).collect()
}

#[test]
fn should_score_a_renamed_copy_at_least_point_nine() {
  let found = code_index().near_duplicates(entity(1), 0.6, 10);
  let renamed = found.iter().find(|m| m.entity == entity(2)).unwrap();
  assert_eq!(renamed.exact_jaccard, 1.0);
  assert_eq!(renamed.estimated_jaccard, 1.0);
  assert_eq!(renamed.size_ratio, 1.0);
  assert_eq!(renamed.query_tokens, renamed.match_tokens);
}

#[test]
fn should_score_reordered_statements_in_the_middle_range() {
  let found = code_index().near_duplicates(entity(1), 0.0, 10);
  let reordered = found.iter().find(|m| m.entity == entity(3)).unwrap();
  assert!(reordered.exact_jaccard > 0.3 && reordered.exact_jaccard < 0.9, "{}", reordered.exact_jaccard);
}

#[test]
fn should_score_unrelated_bodies_below_point_two() {
  let index = code_index();
  let found = index.near_duplicates(entity(4), 0.0, 10);
  assert!(found.iter().all(|m| m.exact_jaccard < 0.2), "{found:?}");
  assert!(index.near_duplicates(entity(4), 0.2, 10).is_empty());
}

#[test]
fn should_ignore_small_bodies_and_report_why() {
  let index = code_index();
  assert_eq!(index.structural_status(entity(5)), StructuralStatus::TooSmall { tokens: lex(TINY).len() });
  assert_eq!(index.structural_status(entity(6)), StructuralStatus::NoBody);
  assert_eq!(index.structural_status(entity(1)), StructuralStatus::Indexed { tokens: lex(ORIGINAL).len() });
  assert_eq!(index.structural_status(entity(99)), StructuralStatus::Unknown);
  assert!(index.near_duplicates(entity(5), 0.0, 10).is_empty());
  assert!(index.near_duplicates_of_tokens(&lex(TINY), 0.0, 10).is_empty());
  assert!(!ids(&index.near_duplicates(entity(1), 0.0, 10)).contains(&5));
}

#[test]
fn should_find_matches_for_code_that_is_not_indexed() {
  let found = code_index().near_duplicates_of_tokens(&lex(RENAMED), 0.9, 10);
  assert_eq!(ids(&found), [1, 2]);
}

#[test]
fn should_be_deterministic_across_independent_builds() {
  let (first, second) = (code_index(), code_index());
  assert_eq!(first.near_duplicates(entity(1), 0.0, 10), second.near_duplicates(entity(1), 0.0, 10));
  assert_eq!(first.to_bytes(), second.to_bytes());
}

#[test]
fn should_limit_results_to_k_ordered_by_exact_jaccard_then_entity() {
  let found = code_index().near_duplicates(entity(1), 0.0, 2);
  assert_eq!(ids(&found), [2, 3]);
  assert!(code_index().near_duplicates(entity(1), 0.0, 0).is_empty());
  assert!(code_index().near_duplicates(entity(1), f64::NAN, 5).is_empty());
}

#[test]
fn should_cluster_verified_pairs_deterministically() {
  let mut index = code_index();
  index.upsert(doc(7, "other", Some(UNRELATED))).unwrap();
  let clusters = index.clusters(0.3);
  assert!(!clusters.sampled);
  let clusters = clusters.clusters;
  assert_eq!(clusters.len(), 2);
  assert_eq!(clusters[0].members, [entity(1), entity(2), entity(3)]);
  assert_eq!(clusters[1].members, [entity(4), entity(7)]);
  assert_eq!(clusters[1].verified_pairs, 1);
  assert_eq!(clusters[1].min_pair_jaccard, 1.0);
  assert_eq!(clusters, code_index_with_other().clusters(0.3).clusters);
}

fn code_index_with_other() -> SimilarityIndex {
  let mut index = code_index();
  index.upsert(doc(7, "other", Some(UNRELATED))).unwrap();
  index
}

#[test]
fn should_not_cluster_when_threshold_exceeds_every_pair_except_exact_copies() {
  let clusters = code_index_with_other().clusters(0.95).clusters;
  assert_eq!(
    clusters.iter().map(|c| c.members.clone()).collect::<Vec<_>>(),
    [vec![entity(1), entity(2)], vec![entity(4), entity(7)]]
  );
}

#[test]
fn should_replace_a_doc_on_upsert_and_forget_it_on_remove() {
  let mut index = code_index();
  index.upsert(doc(2, "handleBatch", Some(UNRELATED))).unwrap();
  assert_eq!(index.len(), 6);
  assert!(!ids(&index.near_duplicates(entity(1), 0.6, 10)).contains(&2));
  assert_eq!(ids(&index.near_duplicates(entity(4), 0.6, 10)), [2]);

  assert!(index.remove(entity(2)));
  assert!(!index.remove(entity(2)));
  assert!(index.near_duplicates(entity(4), 0.0, 10).is_empty());
  assert!(!index.contains(entity(2)));
  index.upsert(doc(8, "reuse", Some(ORIGINAL))).unwrap();
  assert_eq!(ids(&index.near_duplicates(entity(1), 0.9, 10)), [8]);
}

#[test]
fn should_rank_exact_name_hits_above_expansion_only_hits() {
  let mut index = SimilarityIndex::default();
  index.upsert(text_doc(1, "removeUser", None, "src/a.rs")).unwrap();
  index.upsert(text_doc(2, "deleteUser", None, "src/b.rs")).unwrap();
  index.upsert(text_doc(3, "renderPage", None, "src/c.rs")).unwrap();
  let hits = index.search("remove user", &SearchOptions::default());
  assert_eq!(hits.iter().map(|h| h.entity.raw()).collect::<Vec<_>>(), [1, 2]);
  assert!(hits[0].score > hits[1].score);
}

#[test]
fn should_report_synonym_matches_as_via_expansion() {
  let mut index = SimilarityIndex::default();
  index.upsert(text_doc(1, "removeUser", None, "src/a.rs")).unwrap();
  index.upsert(text_doc(2, "deleteUser", None, "src/b.rs")).unwrap();
  let hits = index.search("remove user", &SearchOptions::default());
  assert_eq!(hits[0].direct_terms, ["remov", "user"]);
  assert!(hits[0].via_expansion.is_empty());
  assert_eq!(hits[1].direct_terms, ["user"]);
  assert_eq!(hits[1].via_expansion.len(), 1);
  assert_eq!(hits[1].via_expansion[0].term, "delet");
  assert_eq!(hits[1].via_expansion[0].from, ["remov"]);
  assert_eq!(hits[1].matched_fields.len(), 1);
  assert_eq!(hits[1].matched_fields[0].terms, ["user", "delet"]);
}

#[test]
fn should_match_a_concept_that_shares_no_literal_term() {
  let mut index = SimilarityIndex::default();
  index.upsert(text_doc(1, "authenticateRequest", None, "src/a.rs")).unwrap();
  let hits = index.search("login", &SearchOptions::default());
  assert_eq!(hits.len(), 1);
  assert!(hits[0].direct_terms.is_empty());
  assert_eq!(hits[0].via_expansion[0].term, "authenticat");
}

#[test]
fn should_weight_name_above_doc_above_path() {
  let mut index = SimilarityIndex::default();
  index.upsert(text_doc(1, "unrelated", Some("invoice"), "src/other/file.rs")).unwrap();
  index.upsert(text_doc(2, "invoice", Some("alpha"), "src/other/file.rs")).unwrap();
  index.upsert(text_doc(3, "unrelated", Some("alpha"), "src/invoice/file.rs")).unwrap();
  let hits = index.search("invoice", &SearchOptions::default());
  assert_eq!(hits.iter().map(|h| h.entity.raw()).collect::<Vec<_>>(), [2, 1, 3]);
  assert_eq!(hits[0].matched_fields[0].field, crate::Field::Name);
  assert_eq!(hits[1].matched_fields[0].field, crate::Field::Doc);
  assert_eq!(hits[2].matched_fields[0].field, crate::Field::Path);
}

#[test]
fn should_filter_by_path_prefix_before_taking_the_top_hits() {
  let mut index = SimilarityIndex::default();
  index.upsert(text_doc(1, "priceRules", None, "apps/web-shop/src/price-rules.ts")).unwrap();
  index.upsert(text_doc(2, "priceRulesFor", None, "apps/back-office/src/price-rules.ts")).unwrap();
  index.upsert(text_doc(3, "applyPriceRules", None, "apps/back-office/src/apply.ts")).unwrap();
  let options = SearchOptions {
    k: 1,
    path_prefix: Some("apps/back-office/".to_owned()),
    ..SearchOptions::default()
  };
  let hits = index.search("price rules", &options);
  assert_eq!(hits.iter().map(|h| h.entity.raw()).collect::<Vec<_>>(), [2]);
}

#[test]
fn should_filter_search_by_kind_and_repo() {
  let mut index = SimilarityIndex::default();
  index.upsert(text_doc(1, "parseConfig", None, "src/a.rs")).unwrap();
  index
    .upsert(SimilarityDoc {
      kind: EntityKind::Class,
      ..text_doc(2, "ParseConfig", None, "src/b.rs")
    })
    .unwrap();
  index
    .upsert(SimilarityDoc {
      repo: RepoName::new("other").unwrap(),
      ..text_doc(3, "parseConfig", None, "src/c.rs")
    })
    .unwrap();
  let all = index.search("parse config", &SearchOptions::default());
  assert_eq!(all.len(), 3);
  let functions = index.search(
    "parse config",
    &SearchOptions {
      kinds: vec![EntityKind::Function],
      ..SearchOptions::default()
    },
  );
  assert_eq!(functions.iter().map(|h| h.entity.raw()).collect::<Vec<_>>(), [1, 3]);
  let other = index.search(
    "parse config",
    &SearchOptions {
      repos: vec![RepoName::new("other").unwrap()],
      ..SearchOptions::default()
    },
  );
  assert_eq!(other.iter().map(|h| h.entity.raw()).collect::<Vec<_>>(), [3]);
  assert_eq!(index.search("parse config", &SearchOptions { k: 1, ..SearchOptions::default() }).len(), 1);
}

#[test]
fn should_keep_get_when_it_is_the_only_query_term() {
  let mut index = SimilarityIndex::default();
  index.upsert(text_doc(1, "get", None, "src/a.rs")).unwrap();
  index.upsert(text_doc(2, "getUser", None, "src/b.rs")).unwrap();
  let hits = index.search("get", &SearchOptions::default());
  assert_eq!(hits[0].entity, entity(1));
  assert_eq!(hits[0].direct_terms, ["get"]);
}

#[test]
fn should_return_nothing_for_empty_or_unknown_queries() {
  let mut index = SimilarityIndex::default();
  assert!(index.search("anything", &SearchOptions::default()).is_empty());
  index.upsert(text_doc(1, "alpha", None, "src/a.rs")).unwrap();
  assert!(index.search("", &SearchOptions::default()).is_empty());
  assert!(index.search("zzzz", &SearchOptions::default()).is_empty());
}

fn lexical_corpus() -> Vec<SimilarityDoc> {
  let names = [
    "parseHttpResponse",
    "removeUser",
    "deleteAccount",
    "to_cents",
    "fetchUserProfile",
    "loadConfig",
    "formatMoney",
    "renderInvoice",
    "createMember",
    "insertRow",
  ];
  (0..40)
    .map(|i| {
      let mut d = text_doc(
        i as u64 + 1,
        names[i % names.len()],
        Some(&format!("handles case {} for the {}", i, names[(i * 3) % names.len()])),
        &format!("src/mod{}/file{}.rs", i % 4, i),
      );
      d.referenced_names = vec![names[(i + 1) % names.len()].to_owned(), format!("helper{i}")];
      d.signature = Some(format!("fn {}(arg: Type{}) -> Out", names[i % names.len()], i % 5));
      d
    })
    .collect()
}

#[test]
fn should_produce_identical_scores_incrementally_and_from_a_full_rebuild() {
  let corpus = lexical_corpus();
  let mut incremental = SimilarityIndex::default();
  for d in &corpus {
    incremental.upsert(d.clone()).unwrap();
  }
  let mut final_docs = corpus.clone();
  for id in [3u64, 7, 11, 19] {
    incremental.remove(entity(id));
    final_docs.retain(|d| d.entity != entity(id));
  }
  let mut changed = text_doc(5, "removeAccountToken", Some("completely new text"), "src/new/path.rs");
  changed.referenced_names = vec!["login".to_owned()];
  incremental.upsert(changed.clone()).unwrap();
  final_docs.iter_mut().filter(|d| d.entity == entity(5)).for_each(|d| *d = changed.clone());
  let added = text_doc(100, "deleteUserAccount", None, "src/extra.rs");
  incremental.upsert(added.clone()).unwrap();
  final_docs.push(added);

  let mut rebuilt = SimilarityIndex::default();
  for d in &final_docs {
    rebuilt.upsert(d.clone()).unwrap();
  }
  let options = SearchOptions {
    k: 100,
    ..SearchOptions::default()
  };
  for query in ["remove user", "parse http", "money cents", "login token", "case helper3", "file"] {
    let (a, b) = (incremental.search(query, &options), rebuilt.search(query, &options));
    assert_eq!(a.len(), b.len(), "{query}");
    for (left, right) in a.iter().zip(&b) {
      assert_eq!(left.entity, right.entity, "{query}");
      assert!((left.score - right.score).abs() < 1e-12, "{query}: {} vs {}", left.score, right.score);
      assert_eq!(left.matched_fields, right.matched_fields, "{query}");
    }
  }
}

#[test]
fn should_round_trip_through_bytes() {
  let mut index = code_index();
  for d in lexical_corpus() {
    index
      .upsert(SimilarityDoc {
        entity: EntityId::from_raw(d.entity.raw() + 1_000),
        ..d
      })
      .unwrap();
  }
  index.remove(entity(2));
  let bytes = index.to_bytes();
  let restored = SimilarityIndex::from_bytes(&bytes).unwrap();
  assert_eq!(restored.len(), index.len());
  assert_eq!(restored.config(), index.config());
  assert_eq!(restored.to_bytes(), bytes);
  assert_eq!(restored.near_duplicates(entity(1), 0.0, 10), index.near_duplicates(entity(1), 0.0, 10));
  assert_eq!(restored.structural_status(entity(5)), StructuralStatus::TooSmall { tokens: lex(TINY).len() });
  let options = SearchOptions {
    k: 50,
    ..SearchOptions::default()
  };
  assert_eq!(restored.search("remove user", &options), index.search("remove user", &options));
}

#[test]
fn should_round_trip_an_empty_index_and_a_custom_config() {
  let config = SimilarityConfig {
    bands: 16,
    rows: 8,
    min_tokens: 10,
  };
  let index = SimilarityIndex::new(config).unwrap();
  let restored = SimilarityIndex::from_bytes(&index.to_bytes()).unwrap();
  assert!(restored.is_empty());
  assert_eq!(restored.config(), config);
}

#[test]
fn should_reject_a_foreign_header() {
  assert!(matches!(SimilarityIndex::from_bytes(b"not an index at all"), Err(SimilarityError::BadMagic)));
  assert!(matches!(SimilarityIndex::from_bytes(b""), Err(SimilarityError::BadMagic)));
}

#[test]
fn should_reject_an_unsupported_version() {
  let mut bytes = code_index().to_bytes();
  bytes[8..12].copy_from_slice(&99u32.to_le_bytes());
  assert!(matches!(
    SimilarityIndex::from_bytes(&bytes),
    Err(SimilarityError::UnsupportedVersion { found: 99, expected: 3 })
  ));
}

#[test]
fn should_reject_truncated_and_corrupted_payloads() {
  let bytes = code_index().to_bytes();
  assert!(matches!(SimilarityIndex::from_bytes(&bytes[..10]), Err(SimilarityError::Truncated)));
  assert!(matches!(SimilarityIndex::from_bytes(&bytes[..bytes.len() - 20]), Err(SimilarityError::ChecksumMismatch)));
  let mut flipped = bytes.clone();
  flipped[40] ^= 0xff;
  assert!(matches!(SimilarityIndex::from_bytes(&flipped), Err(SimilarityError::ChecksumMismatch)));
}

#[test]
fn should_reject_a_valid_checksum_over_an_invalid_config() {
  let mut bytes = code_index().to_bytes();
  bytes[12..20].copy_from_slice(&0u64.to_le_bytes());
  let payload_len = bytes.len() - 8;
  let checksum = crate::hashing::fnv1a(&bytes[..payload_len]);
  bytes[payload_len..].copy_from_slice(&checksum.to_le_bytes());
  assert!(matches!(SimilarityIndex::from_bytes(&bytes), Err(SimilarityError::Corrupt { .. })));
}

#[test]
fn should_validate_the_lsh_configuration() {
  assert!(
    SimilarityIndex::new(SimilarityConfig {
      bands: 0,
      rows: 4,
      min_tokens: 40
    })
    .is_err()
  );
  assert!(
    SimilarityIndex::new(SimilarityConfig {
      bands: 33,
      rows: 4,
      min_tokens: 40
    })
    .is_err()
  );
  assert!(
    SimilarityIndex::new(SimilarityConfig {
      bands: usize::MAX,
      rows: 2,
      min_tokens: 40
    })
    .is_err()
  );
  assert!(
    SimilarityIndex::new(SimilarityConfig {
      bands: 64,
      rows: 2,
      min_tokens: 40
    })
    .is_ok()
  );
}

#[test]
fn should_clamp_min_tokens_to_the_shingle_size() {
  let mut index = SimilarityIndex::new(SimilarityConfig {
    bands: 32,
    rows: 4,
    min_tokens: 0,
  })
  .unwrap();
  index.upsert(doc(1, "tiny", Some("fn f"))).unwrap();
  assert!(matches!(index.structural_status(entity(1)), StructuralStatus::TooSmall { .. }));
}

#[test]
fn should_sample_oversized_buckets_and_report_it() {
  let mut index = SimilarityIndex::default();
  for id in 1..=600u64 {
    index.upsert(doc(id, "generated", Some(ORIGINAL))).unwrap();
  }
  let report = index.clusters(0.9);
  assert!(report.sampled);
  assert_eq!(report.oversized_buckets, 32);
  assert_eq!(report.clusters.len(), 1);
  assert_eq!(report.clusters[0].members.len(), 600);
  assert_eq!(report.clusters[0].min_pair_jaccard, 1.0);
  assert_eq!(report, index.clusters(0.9));
}

#[test]
fn should_reject_term_frequencies_in_zero_length_fields() {
  let mut index = SimilarityIndex::default();
  index.upsert(text_doc(1, "alpha", None, "src/a.rs")).unwrap();
  let mut bytes = index.to_bytes();
  let path = b"src/a.rs";
  let lens_start = bytes.windows(path.len()).position(|w| w == path).unwrap() + path.len();
  bytes[lens_start..lens_start + 4].copy_from_slice(&0u32.to_le_bytes());
  let payload_len = bytes.len() - 8;
  let checksum = crate::hashing::fnv1a(&bytes[..payload_len]);
  bytes[payload_len..].copy_from_slice(&checksum.to_le_bytes());
  assert!(matches!(SimilarityIndex::from_bytes(&bytes), Err(SimilarityError::Corrupt { .. })));
}

fn synthetic(len: usize) -> Vec<u32> {
  (0..len as u32).map(|i| 16 + (i.wrapping_mul(2_654_435_761) >> 7) % 97).collect()
}

fn token_doc(id: u64, tokens: Vec<u32>) -> SimilarityDoc {
  SimilarityDoc {
    body: Some(BodyFingerprintInput {
      token_count: tokens.len() as u32,
      line_count: 1,
      normalized_tokens: tokens,
    }),
    ..doc(id, "synthetic", None)
  }
}

fn small_index(docs: Vec<Vec<u32>>) -> SimilarityIndex {
  let mut index = SimilarityIndex::default();
  for (position, tokens) in docs.into_iter().enumerate() {
    index.upsert(token_doc(position as u64 + 1, tokens)).unwrap();
  }
  index
}

#[test]
fn should_index_bodies_from_the_default_minimum_up_to_the_small_body_limit() {
  let index = small_index(vec![synthetic(23), synthetic(24), synthetic(39), synthetic(40)]);
  assert_eq!(index.structural_status(entity(1)), StructuralStatus::TooSmall { tokens: 23 });
  assert_eq!(index.structural_status(entity(2)), StructuralStatus::Indexed { tokens: 24 });
  assert_eq!(index.structural_status(entity(3)), StructuralStatus::Indexed { tokens: 39 });
  assert_eq!(index.structural_status(entity(4)), StructuralStatus::Indexed { tokens: 40 });
}

#[test]
fn should_find_identical_small_bodies_and_score_them_one() {
  let index = small_index(vec![synthetic(30), synthetic(30), synthetic(39)]);
  let found = index.near_duplicates(entity(1), 0.0, 10);
  assert_eq!(ids(&found), vec![2]);
  assert_eq!(found[0].exact_jaccard, 1.0);
  assert_eq!((found[0].query_tokens, found[0].match_tokens), (30, 30));
}

#[test]
fn should_apply_a_higher_floor_to_small_pairs_than_the_caller_threshold() {
  let base = synthetic(30);
  let mut late = base.clone();
  late[29] = 900;
  let mut middle = base.clone();
  middle[15] = 900;
  let index = small_index(vec![base, late, middle]);
  let found = index.near_duplicates(entity(1), 0.1, 10);
  assert_eq!(ids(&found), vec![2]);
  assert!(found[0].exact_jaccard >= 0.85 && found[0].exact_jaccard < 1.0, "{}", found[0].exact_jaccard);
  assert!(index.near_duplicates(entity(1), 0.1, 10).iter().all(|found| found.entity != entity(3)));
  assert!(index.clusters(0.1).clusters.iter().all(|cluster| !cluster.members.contains(&entity(3))));
}

#[test]
fn should_keep_the_caller_threshold_when_it_exceeds_the_small_floor() {
  let base = synthetic(30);
  let mut late = base.clone();
  late[29] = 900;
  let index = small_index(vec![base, late]);
  assert!(index.near_duplicates(entity(1), 0.99, 10).is_empty());
  assert_eq!(index.near_duplicates(entity(1), 0.9, 10).len(), 1);
}

#[test]
fn should_never_compare_small_bodies_with_large_ones() {
  let large = synthetic(60);
  let small = large[..30].to_vec();
  let index = small_index(vec![small.clone(), large.clone()]);
  assert!(index.near_duplicates(entity(1), 0.0, 10).is_empty());
  assert!(index.near_duplicates(entity(2), 0.0, 10).is_empty());
  assert!(index.near_duplicates_of_tokens(&large, 0.0, 10).iter().all(|found| found.entity != entity(1)));
  assert!(index.clusters(0.0).clusters.is_empty());
}

#[test]
fn should_still_use_five_token_shingles_for_large_bodies() {
  let base = synthetic(60);
  let mut changed = base.clone();
  changed[59] = 900;
  let index = small_index(vec![base, changed]);
  let found = index.near_duplicates(entity(1), 0.5, 10);
  assert_eq!(ids(&found), vec![2]);
  assert!((found[0].exact_jaccard - 55.0 / 57.0).abs() < 1e-9, "{}", found[0].exact_jaccard);
}

#[test]
fn should_reject_low_entropy_small_bodies() {
  let repetitive: Vec<u32> = (0..30).map(|i| 16 + i % 2).collect();
  let few_shapes: Vec<u32> = (0..30).map(|i| 16 + (i / 10) % 4).collect();
  let index = small_index(vec![repetitive.clone(), few_shapes, synthetic(30)]);
  assert_eq!(index.structural_status(entity(1)), StructuralStatus::LowEntropy { tokens: 30 });
  assert_eq!(index.structural_status(entity(2)), StructuralStatus::LowEntropy { tokens: 30 });
  assert_eq!(index.structural_status(entity(3)), StructuralStatus::Indexed { tokens: 30 });
  assert!(index.near_duplicates_of_tokens(&repetitive, 0.0, 10).is_empty());
  let restored = SimilarityIndex::from_bytes(&index.to_bytes()).unwrap();
  assert_eq!(restored.structural_status(entity(1)), StructuralStatus::LowEntropy { tokens: 30 });
}

#[test]
fn should_flag_small_matches_and_clusters_and_expose_verified_pairs() {
  let index = small_index(vec![synthetic(30), synthetic(30), synthetic(80), synthetic(80)]);
  let small = index.near_duplicates(entity(1), 0.0, 10);
  assert_eq!((small[0].small, small[0].shingle_size), (true, 3));
  let large = index.near_duplicates(entity(3), 0.0, 10);
  assert_eq!((large[0].small, large[0].shingle_size), (false, 5));
  let flags: Vec<bool> = index.clusters(0.0).clusters.iter().map(|cluster| cluster.small).collect();
  assert_eq!(flags.len(), 2);
  assert!(flags.contains(&true) && flags.contains(&false));
  let pairs = index.structural_pairs(0.0);
  assert_eq!(pairs.len(), 2);
  assert!(
    pairs
      .iter()
      .any(|pair| pair.small && pair.left == entity(1) && pair.right == entity(2) && pair.jaccard == 1.0)
  );
  assert!(pairs.iter().any(|pair| !pair.small && pair.left == entity(3) && pair.right == entity(4)));
}

#[test]
fn should_only_report_pairs_that_were_verified_in_a_transitive_chain() {
  let base = synthetic(30);
  let mut middle = base.clone();
  middle[29] = 900;
  let mut far = middle.clone();
  far[0] = 901;
  far[1] = 902;
  let index = small_index(vec![base, middle, far]);
  let pairs = index.structural_pairs(0.0);
  assert!(pairs.iter().all(|pair| pair.jaccard >= 0.85));
  assert_eq!(index.clusters(0.0).clusters[0].members.len(), 3);
  assert!(!pairs.iter().any(|pair| pair.left == entity(1) && pair.right == entity(3)));
}

#[test]
fn should_persist_the_shingle_size_and_restore_small_and_large_docs_identically() {
  let index = small_index(vec![synthetic(30), synthetic(30), synthetic(80), synthetic(80)]);
  let bytes = index.to_bytes();
  let restored = SimilarityIndex::from_bytes(&bytes).unwrap();
  assert_eq!(restored.to_bytes(), bytes);
  assert_eq!(restored.near_duplicates(entity(1), 0.0, 10), index.near_duplicates(entity(1), 0.0, 10));
  assert_eq!(restored.near_duplicates(entity(3), 0.0, 10), index.near_duplicates(entity(3), 0.0, 10));
  assert_eq!(ids(&restored.near_duplicates(entity(1), 0.0, 10)), vec![2]);
}

#[test]
fn should_reject_a_version_one_blob_so_the_index_is_rebuilt() {
  let mut bytes = small_index(vec![synthetic(30)]).to_bytes();
  bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
  assert!(matches!(
    SimilarityIndex::from_bytes(&bytes),
    Err(SimilarityError::UnsupportedVersion { found: 1, expected: 3 })
  ));
}

#[test]
fn should_reject_a_shingle_size_that_does_not_match_the_body_size() {
  let mut bytes = small_index(vec![synthetic(30)]).to_bytes();
  let mut marker = 30u64.to_le_bytes().to_vec();
  marker.extend(3u64.to_le_bytes());
  let at = bytes.windows(marker.len()).position(|window| window == marker).unwrap() + 8;
  bytes[at..at + 8].copy_from_slice(&5u64.to_le_bytes());
  let payload_len = bytes.len() - 8;
  let checksum = crate::hashing::fnv1a(&bytes[..payload_len]);
  bytes[payload_len..].copy_from_slice(&checksum.to_le_bytes());
  assert!(matches!(SimilarityIndex::from_bytes(&bytes), Err(SimilarityError::Corrupt { .. })));
}

#[test]
fn should_drop_generic_words_from_name_words() {
  assert_eq!(crate::name_words("formatMoney").into_iter().collect::<Vec<_>>(), vec!["format", "money"]);
  assert_eq!(crate::name_words("to_cents_v2").into_iter().collect::<Vec<_>>(), vec!["cents"]);
  assert!(crate::name_words("getItemValue").is_empty());
}

#[test]
fn should_not_relate_names_that_only_share_a_generic_word() {
  assert!(!crate::names_related("getUser", "getOrder"));
  assert!(!crate::names_related("new_foo", "new_bar"));
  assert!(!crate::names_related("isAdmin", "isArchived"));
  assert!(!crate::names_related("get_by_id", "set_by_id"));
  assert!(!crate::names_related("get_by_id", "get_by_name"));
  assert!(!crate::names_related("get_with_id", "new_with_id"));
}

#[test]
fn should_relate_names_that_share_a_meaningful_word_or_two_words() {
  assert!(crate::names_related("getUserName", "loadUserName"));
  assert!(crate::names_related("formatMoney", "formatCents"));
  assert!(crate::names_related("getItem", "get_item"));
}
