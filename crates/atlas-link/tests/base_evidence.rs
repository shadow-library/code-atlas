mod common;

use std::collections::BTreeMap;

use atlas_core::EdgeKind;
use common::{Indexed, index_tree, pass_facts, write_tree};
use serde_json::json;

fn express(name: &str, port: u32, routes: &str) -> Vec<(String, String)> {
  vec![
    (
      format!("{name}/package.json"),
      format!(r#"{{ "name": "{name}", "version": "1.0.0", "scripts": {{ "start": "node s.js" }}, "dependencies": {{ "express": "^4.0.0" }} }}"#),
    ),
    (
      format!("{name}/src/server.ts"),
      format!("import express from 'express';\nconst app = express();\n{routes}\napp.listen({port});\n"),
    ),
  ]
}

const WEB: &str = r#"
export async function envApi(id: string) { return fetch(`${process.env.API_URL}/users/${id}`); }
export async function envNone(id: string) { return fetch(`${process.env.SERVICE_URL}/users/${id}`); }
export async function envBilling(id: string) { return fetch(`${process.env.BILLING_API_URL}/users/${id}`); }
export async function envApiKeyOnly(id: string) { return fetch(`${process.env.PAYMENTS_API_KEY_URL}/users/${id}`); }
export async function viteApi(id: string) { return fetch(`${import.meta.env.VITE_API_URL}/users/${id}`); }
export async function literalHost(id: string) { return fetch(`https://api.example.com/users/${id}`); }
export async function v1Users(id: string) { return fetch(`${process.env.SERVICE_URL}/v1/users/${id}`); }
"#;

fn project(scratch: &std::path::Path) -> Indexed {
  let source = scratch.join("source");
  let shared = "app.get('/health', (_q, r) => r.send('ok'));\napp.get('/users/:id', (_q, r) => r.send('u'));\n";
  let mut files = Vec::new();
  files.extend(express("api", 3000, shared));
  files.extend(express("users-api", 3001, shared));
  files.extend(express("billing-api", 3002, shared));
  files.extend(express("legacy", 3003, "app.get('/v1/users/:id', (_q, r) => r.send('legacy'));"));
  files.push(("web/package.json".into(), r#"{ "name": "web", "version": "1.0.0", "dependencies": {} }"#.into()));
  files.push(("web/src/c.ts".into(), WEB.into()));
  let refs: Vec<(&str, &str)> = files.iter().map(|(path, content)| (path.as_str(), content.as_str())).collect();
  write_tree(&source, &refs);
  index_tree(scratch, "p", &source, &["api", "users-api", "billing-api", "legacy", "web"])
}

fn links(indexed: &Indexed) -> BTreeMap<String, BTreeMap<String, (f32, serde_json::Value)>> {
  let facts = pass_facts(&indexed.engine);
  indexed
    .engine
    .store()
    .read(|tx| -> Result<_, atlas_store::StoreError> {
      let mut links: BTreeMap<String, BTreeMap<String, (f32, serde_json::Value)>> = BTreeMap::new();
      for edge in facts.edges.iter().filter(|edge| edge.kind == EdgeKind::CallsEndpoint) {
        let from = tx.entity(edge.from)?.unwrap().name;
        let to = tx.entity(edge.to)?.unwrap().key.to_string();
        links
          .entry(from)
          .or_default()
          .insert(to, (edge.certainty.confidence().unwrap().value(), edge.attrs["base_evidence"].clone()));
      }
      Ok(links)
    })
    .unwrap()
}

fn confidences(links: &BTreeMap<String, BTreeMap<String, (f32, serde_json::Value)>>, caller: &str) -> Vec<(String, f32)> {
  links[caller].iter().map(|(endpoint, (confidence, _))| (endpoint.clone(), *confidence)).collect()
}

#[test]
fn should_rank_named_base_evidence_above_noise_third_party_hosts_and_no_evidence() {
  let scratch = tempfile::tempdir().unwrap();
  let links = links(&project(scratch.path()));
  let no_evidence = confidences(&links, "envNone");
  assert_eq!(
    no_evidence.iter().map(|(endpoint, _)| endpoint.as_str()).collect::<Vec<_>>(),
    ["endpoint:api:GET /users/{id}", "endpoint:billing-api:GET /users/{id}", "endpoint:users-api:GET /users/{id}"]
  );
  for noise in ["envApi", "envApiKeyOnly", "viteApi", "literalHost"] {
    assert_eq!(confidences(&links, noise), no_evidence, "{noise}");
    assert!(links[noise].values().all(|(_, evidence)| *evidence == json!([])), "{noise}");
  }
  let named = &links["envBilling"];
  let billing = &named["endpoint:billing-api:GET /users/{id}"];
  assert_eq!(billing.1, json!(["billing-api"]));
  let unnamed = no_evidence.iter().map(|(_, confidence)| *confidence).fold(0.0_f32, f32::max);
  assert!(billing.0 > unnamed, "{billing:?} vs {unnamed}");
  assert!(
    named
      .iter()
      .filter(|(endpoint, _)| !endpoint.starts_with("endpoint:billing-api"))
      .all(|(_, (confidence, _))| *confidence < unnamed)
  );
}

#[test]
fn should_skip_base_path_prefix_candidates_when_full_matches_exist() {
  let scratch = tempfile::tempdir().unwrap();
  let links = links(&project(scratch.path()));
  assert!(links["envNone"].keys().all(|endpoint| !endpoint.starts_with("endpoint:legacy")));
  assert_eq!(links["v1Users"].keys().collect::<Vec<_>>(), ["endpoint:legacy:GET /v1/users/{id}"]);
}
