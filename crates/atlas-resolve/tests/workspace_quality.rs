mod common;

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::time::Instant;

use atlas_core::{CertaintyClass, EntityId, EntityKey, EntityKind, RefKind, UnresolvedReason};
use atlas_project::{PackageGraph, RepoConfig, discover};
use atlas_resolve::{OutcomeCounts, ProjectSymbolTable, resolve_file};

fn percent(part: u32, total: u32) -> f64 {
  if total == 0 { 0.0 } else { f64::from(part) * 100.0 / f64::from(total) }
}

fn reason_label(reason: &UnresolvedReason) -> String {
  match reason {
    UnresolvedReason::NotFound => "not_found".to_owned(),
    UnresolvedReason::Ambiguous { .. } => "ambiguous".to_owned(),
    UnresolvedReason::Dynamic => "dynamic".to_owned(),
    UnresolvedReason::External { package } => format!("external:{package}"),
    UnresolvedReason::Unsupported { detail } => format!("unsupported:{detail}"),
    _ => "other".to_owned(),
  }
}

#[test]
#[ignore = "measures resolution quality and speed on this workspace; run with --release -- --ignored --nocapture"]
fn should_report_resolution_quality_on_this_workspace() {
  let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
  let repo = common::repo("code-atlas");
  let config = RepoConfig {
    name: repo.clone(),
    path: root.clone(),
    include: Vec::new(),
    exclude: vec!["/fixtures".to_owned(), "/docs".to_owned(), "/scratch-pad".to_owned()],
    languages: None,
  };
  let discovered = discover(&config, 2 * 1024 * 1024).unwrap();
  let graph = PackageGraph::load(&root, &discovered.files);
  let sources: Vec<_> = discovered.files.iter().filter(|file| file.language.is_some()).collect();

  let started = Instant::now();
  let corpus: Vec<_> = sources
    .iter()
    .map(|file| {
      let source = std::fs::read_to_string(root.join(file.path.as_str())).unwrap();
      (common::analyze(&repo, file.path.as_str(), &source), common::key_context(&graph, &file.path))
    })
    .collect();
  let analyzed = started.elapsed();

  let started = Instant::now();
  let mut table = ProjectSymbolTable::new();
  table.set_repo_packages(repo.clone(), graph);
  for (facts, context) in &corpus {
    table.upsert_file(facts, context).unwrap();
  }
  let relocations = table.refresh();
  let indexed = started.elapsed();

  let started = Instant::now();
  let mut by_kind: BTreeMap<RefKind, OutcomeCounts> = BTreeMap::new();
  let mut reasons: BTreeMap<(RefKind, String), u32> = BTreeMap::new();
  let mut edges = 0usize;
  let mut const_edges = 0u32;
  let mut const_weakened = 0u32;
  for (facts, context) in &corpus {
    let resolved = resolve_file(facts, &table).unwrap();
    edges += resolved.edges.len() + resolved.structural.len();
    let constants: HashSet<EntityId> = facts
      .symbol_keys(context)
      .unwrap()
      .into_iter()
      .zip(&facts.symbols)
      .filter(|(_, symbol)| symbol.kind == EntityKind::Constant)
      .map(|(key, _)| EntityId::of(&EntityKey::Symbol(key)))
      .collect();
    for edge in resolved.edges.iter().filter(|edge| constants.contains(&edge.from)) {
      const_edges += 1;
      const_weakened += u32::from(edge.certainty.class() == CertaintyClass::Heuristic);
    }
    for (kind, counts) in &resolved.stats.by_kind {
      by_kind.entry(*kind).or_default().add(counts);
    }
    for unresolved in &resolved.unresolved {
      *reasons.entry((unresolved.kind, reason_label(&unresolved.reason))).or_default() += 1;
    }
  }
  let resolution = started.elapsed();

  let files = corpus.len() as u32;
  eprintln!("files={files} relocations={} edges={edges}", relocations.len());
  eprintln!(
    "analyze={analyzed:?} index+refresh={indexed:?} resolve={resolution:?} resolve/file={:?}",
    resolution / files.max(1)
  );
  let mut total = OutcomeCounts::default();
  eprintln!("{:<14} {:>7} {:>9} {:>10} {:>11}", "kind", "refs", "resolved", "heuristic", "unresolved");
  for (kind, counts) in &by_kind {
    total.add(counts);
    eprintln!(
      "{:<14} {:>7} {:>8.1}% {:>9.1}% {:>10.1}%",
      format!("{kind:?}"),
      counts.total(),
      percent(counts.resolved, counts.total()),
      percent(counts.heuristic, counts.total()),
      percent(counts.unresolved, counts.total())
    );
  }
  eprintln!(
    "{:<14} {:>7} {:>8.1}% {:>9.1}% {:>10.1}%",
    "all",
    total.total(),
    percent(total.resolved, total.total()),
    percent(total.heuristic, total.total()),
    percent(total.unresolved, total.total())
  );
  eprintln!("const/static-originated edges={const_edges} heuristic={const_weakened}");
  let mut top: Vec<_> = reasons.into_iter().collect();
  top.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
  for ((kind, reason), count) in top.into_iter().take(25) {
    eprintln!("  unresolved {kind:?} {reason}: {count}");
  }
  assert!(total.resolved > 0);
  assert_eq!(total.total() as usize, corpus.iter().map(|(facts, _)| facts.references.len()).sum::<usize>());
}
