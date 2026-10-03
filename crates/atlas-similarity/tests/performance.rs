use std::collections::HashSet;
use std::time::{Duration, Instant};

use atlas_core::{BodyFingerprintInput, EntityId, EntityKind, RelPath, RepoName};
use atlas_similarity::{SearchOptions, SimilarityDoc, SimilarityIndex};

const DOC_COUNT: usize = 100_000;
const PLANTED: usize = 2_000;
const VOCAB: u32 = 600;
const VERBS: &[&str] = &[
  "remove", "delete", "fetch", "load", "create", "insert", "parse", "decode", "format", "render", "update", "validate", "send", "handle", "list", "hash", "retry", "check",
  "build", "merge",
];

struct Rng(u64);

impl Rng {
  fn next(&mut self) -> u64 {
    self.0 ^= self.0 >> 12;
    self.0 ^= self.0 << 25;
    self.0 ^= self.0 >> 27;
    self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
  }

  fn below(&mut self, bound: u64) -> u64 {
    self.next() % bound
  }

  fn unit(&mut self) -> f64 {
    (self.next() >> 11) as f64 / (1u64 << 53) as f64
  }
}

fn noun(index: u64) -> String {
  const SYLLABLES: &[&str] = &["ka", "to", "mi", "re", "su", "no", "la", "vi", "pe", "do"];
  format!(
    "{}{}{}",
    SYLLABLES[(index % 10) as usize],
    SYLLABLES[(index / 10 % 10) as usize],
    SYLLABLES[(index / 100 % 10) as usize]
  )
}

fn tokens(rng: &mut Rng) -> Vec<u32> {
  let len = 60 + rng.below(140) as usize;
  (0..len).map(|_| 16 + rng.below(u64::from(VOCAB)) as u32).collect()
}

fn mutate(rng: &mut Rng, original: &[u32], rate: f64) -> Vec<u32> {
  original
    .iter()
    .map(|&token| if rng.unit() < rate { 16 + rng.below(u64::from(VOCAB)) as u32 } else { token })
    .collect()
}

fn body(tokens: Vec<u32>) -> BodyFingerprintInput {
  BodyFingerprintInput {
    token_count: tokens.len() as u32,
    line_count: (tokens.len() / 8) as u32,
    normalized_tokens: tokens,
  }
}

fn doc(rng: &mut Rng, id: u64, tokens: Vec<u32>) -> SimilarityDoc {
  let (verb, first, second) = (VERBS[rng.below(VERBS.len() as u64) as usize], noun(rng.below(300)), noun(rng.below(300)));
  let mut capitalised = first.clone();
  capitalised[..1].make_ascii_uppercase();
  let name = format!("{verb}{capitalised}{second}");
  SimilarityDoc {
    entity: EntityId::from_raw(id + 1),
    kind: EntityKind::Function,
    repo: RepoName::new("repo").unwrap(),
    path: RelPath::new(&format!("src/{first}/{second}.rs")).unwrap(),
    signature: Some(format!("fn {name}(input: {first}) -> {second}")),
    doc: Some(format!("{verb} the {first} for a {second}")),
    referenced_names: vec![noun(rng.below(300)), noun(rng.below(300)), format!("{verb}All")],
    name,
    body: Some(body(tokens)),
  }
}

fn shingle_set(tokens: &[u32]) -> HashSet<[u32; 5]> {
  tokens.windows(5).map(|w| [w[0], w[1], w[2], w[3], w[4]]).collect()
}

fn jaccard(a: &HashSet<[u32; 5]>, b: &HashSet<[u32; 5]>) -> f64 {
  a.intersection(b).count() as f64 / a.union(b).count() as f64
}

fn percentile(samples: &mut [Duration], p: f64) -> Duration {
  samples.sort_unstable();
  samples[((samples.len() - 1) as f64 * p).round() as usize]
}

#[test]
#[ignore]
fn should_meet_the_performance_targets_on_100k_docs() {
  let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
  let mut bodies: Vec<Vec<u32>> = (0..DOC_COUNT - PLANTED).map(|_| tokens(&mut rng)).collect();
  let mut planted: Vec<(u64, u64, f64)> = Vec::new();
  for source in 0..PLANTED {
    let rate = 0.05 + 0.10 * rng.unit();
    let mutated = mutate(&mut rng, &bodies[source], rate);
    let truth = jaccard(&shingle_set(&bodies[source]), &shingle_set(&mutated));
    planted.push((source as u64 + 1, bodies.len() as u64 + 1, truth));
    bodies.push(mutated);
  }
  let docs: Vec<SimilarityDoc> = bodies.into_iter().enumerate().map(|(id, tokens)| doc(&mut rng, id as u64, tokens)).collect();

  let started = Instant::now();
  let mut index = SimilarityIndex::default();
  for d in docs {
    index.upsert(d).unwrap();
  }
  let build = started.elapsed();
  let memory = index.estimated_memory();

  let mut structural_latency = Vec::new();
  let mut found_at_06 = 0;
  let mut eligible_06 = 0;
  let mut found_at_03 = 0;
  let mut eligible_03 = 0;
  for &(source, mutant, truth) in &planted {
    let started = Instant::now();
    let matches = index.near_duplicates(EntityId::from_raw(mutant), 0.3, 10);
    structural_latency.push(started.elapsed());
    let hit = matches.iter().find(|m| m.entity == EntityId::from_raw(source));
    if truth >= 0.3 {
      eligible_03 += 1;
      found_at_03 += usize::from(hit.is_some());
    }
    if truth >= 0.6 {
      eligible_06 += 1;
      found_at_06 += usize::from(hit.is_some_and(|m| m.exact_jaccard >= 0.6));
    }
  }

  let mut search_latency = Vec::new();
  let mut search_hits = 0;
  for _ in 0..2_000 {
    let query = format!("{} {}", VERBS[rng.below(VERBS.len() as u64) as usize], noun(rng.below(300)));
    let started = Instant::now();
    search_hits += index.search(&query, &SearchOptions::default()).len();
    search_latency.push(started.elapsed());
  }

  let recall_06 = found_at_06 as f64 / eligible_06 as f64;
  let (near_p50, near_p95) = (percentile(&mut structural_latency, 0.5), percentile(&mut structural_latency, 0.95));
  let (search_p50, search_p95) = (percentile(&mut search_latency, 0.5), percentile(&mut search_latency, 0.95));
  eprintln!("docs: {DOC_COUNT}");
  eprintln!("build: {build:?}");
  eprintln!(
    "memory estimate: {:.1} MiB (structural {:.1}, lexical {:.1}, metadata {:.1})",
    memory.total() as f64 / 1048576.0,
    memory.structural_bytes as f64 / 1048576.0,
    memory.lexical_bytes as f64 / 1048576.0,
    memory.metadata_bytes as f64 / 1048576.0
  );
  eprintln!("near_duplicates p50: {near_p50:?} p95: {near_p95:?}");
  eprintln!("search p50: {search_p50:?} p95: {search_p95:?} (mean hits {:.1})", search_hits as f64 / 2_000.0);
  eprintln!("planted pairs with exact jaccard >= 0.6: {eligible_06}/{PLANTED}, recall at 0.6: {recall_06:.4}");
  eprintln!(
    "planted pairs with exact jaccard >= 0.3: {eligible_03}/{PLANTED}, recall at 0.3: {:.4}",
    found_at_03 as f64 / eligible_03 as f64
  );

  assert!(build < Duration::from_secs(10), "build {build:?}");
  assert!(near_p95 < Duration::from_millis(10), "near_duplicates p95 {near_p95:?}");
  assert!(search_p95 < Duration::from_millis(10), "search p95 {search_p95:?}");
  assert!(recall_06 >= 0.95, "recall {recall_06}");
}
