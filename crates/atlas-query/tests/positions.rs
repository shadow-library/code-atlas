mod common;

use atlas_core::{ContentHash, EdgeKind, EntityId, EntityKey, EntityKind, LanguageFamily};
use atlas_query::{Site, VerifiedSource};
use atlas_store::EdgeFilter;

#[test]
fn should_locate_the_name_column_of_every_users_service_find_one_call_site() {
  let fixture = common::acme();
  let engine = fixture.query.engine();
  engine.ensure_fresh().unwrap();
  let target = EntityId::of(&"ts:acme-api:acme-api:src/users/users.service::UsersService::findOne#method".parse::<EntityKey>().unwrap());

  let edges = engine
    .store()
    .read(|tx| {
      tx.edges_to(
        target,
        &EdgeFilter {
          kinds: vec![EdgeKind::Calls],
          ..Default::default()
        },
      )
    })
    .unwrap();

  let mut sites = Vec::new();
  for evidence in edges.iter().flat_map(|edge| &edge.evidence) {
    let content = std::fs::read(fixture.repo_root(evidence.repo.as_str()).join(evidence.path.as_str())).unwrap();
    let source = VerifiedSource::verify(&content, &evidence.content_hash, LanguageFamily::Ts).unwrap();
    let site = Site::of(&EdgeKind::Calls, evidence.span.text(source.text()).unwrap(), &EntityKind::Method);
    assert_eq!(site, Site::Member);
    let found = source.name_token(&evidence.span, "findOne", site).unwrap();
    assert_eq!(found.start_line, evidence.span.start_line);
    assert_eq!(&content[found.start_byte as usize..found.end_byte as usize], b"findOne");
    sites.push((evidence.path.to_string(), found.start_line, found.start_col));
  }
  sites.sort();

  assert_eq!(
    sites,
    vec![
      ("src/users/users.controller.ts".to_owned(), 16, 29),
      ("src/users/users.service.spec.ts".to_owned(), 15, 27),
      ("src/users/users.service.spec.ts".to_owned(), 20, 27),
      ("src/users/users.service.ts".to_owned(), 29, 28),
    ]
  );
}

#[test]
fn should_refuse_to_locate_a_name_when_the_file_changed_since_indexing() {
  let fixture = common::acme();
  let engine = fixture.query.engine();
  engine.ensure_fresh().unwrap();
  let target = EntityId::of(&"ts:acme-api:acme-api:src/users/users.service::UsersService::findOne#method".parse::<EntityKey>().unwrap());
  let edges = engine.store().read(|tx| tx.edges_to(target, &EdgeFilter::default())).unwrap();
  let evidence = edges.iter().flat_map(|edge| &edge.evidence).next().unwrap();
  let mut content = std::fs::read(fixture.repo_root(evidence.repo.as_str()).join(evidence.path.as_str())).unwrap();
  content.push(b'\n');

  assert_ne!(ContentHash::of(&content), evidence.content_hash);
  assert!(VerifiedSource::verify(&content, &evidence.content_hash, LanguageFamily::Ts).is_none());
}
