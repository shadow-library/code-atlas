use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::{Certainty, Confidence, ContentHash, EdgeKind, EntityId, Evidence, Freshness, Owner, Producer, ProjectName, RelPath, RepoName, Span};
use atlas_engine::{Engine, EngineOptions, PASS_LINEAGE};
use atlas_project::{AtlasHome, Project, ProjectSettings, RepoConfig};
use atlas_store::{FreshnessPolicy, InferredQuery, NewInferredFact};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

use super::corpus::copy_tree;
use super::error::{QualityError, QualityResult};
use super::selector::{Binding, bind};
use super::snapshot::Snapshot;
use super::truth::Selector;

const ACME_REPOS: [&str; 4] = ["acme-api", "acme-sdk", "acme-web", "acme-billing"];
const MIN_LINEAGE_CONFIDENCE: f32 = 0.85;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
  pub name: String,
  pub passed: bool,
  pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenarioReport {
  pub name: String,
  pub description: String,
  pub passed: bool,
  pub checks: Vec<Check>,
  pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefactorReport {
  pub scenarios: Vec<ScenarioReport>,
}

impl RefactorReport {
  pub fn passed(&self) -> usize {
    self.scenarios.iter().filter(|scenario| scenario.passed).count()
  }
}

#[derive(Clone, Copy)]
struct Subject {
  repo: &'static str,
  path: &'static str,
  symbol: &'static str,
}

struct Scenario {
  name: &'static str,
  description: &'static str,
  changed: Subject,
  edit: fn(&Tree) -> QualityResult<()>,
  deltas: fn(&Before, &Snapshot, &mut Vec<Check>),
}

struct Tree {
  root: PathBuf,
}

impl Tree {
  fn path(&self, repo: &str, path: &str) -> PathBuf {
    self.root.join(repo).join(path)
  }

  fn read(&self, repo: &str, path: &str) -> QualityResult<String> {
    let target = self.path(repo, path);
    fs::read_to_string(&target).map_err(|error| QualityError::io(target, error))
  }

  fn write(&self, repo: &str, path: &str, content: &str) -> QualityResult<()> {
    let target = self.path(repo, path);
    if let Some(parent) = target.parent() {
      fs::create_dir_all(parent).map_err(|error| QualityError::io(parent, error))?;
    }
    fs::write(&target, content).map_err(|error| QualityError::io(target, error))
  }

  fn replace(&self, repo: &str, path: &str, from: &str, to: &str) -> QualityResult<()> {
    let text = self.read(repo, path)?;
    if !text.contains(from) {
      return Err(QualityError::scenario("edit", format!("`{from}` does not occur in {repo}/{path}")));
    }
    self.write(repo, path, &text.replace(from, to))
  }

  fn relocate(&self, repo: &str, from: &str, to: &str) -> QualityResult<()> {
    let content = self.read(repo, from)?;
    self.write(repo, to, &content)?;
    let source = self.path(repo, from);
    fs::remove_file(&source).map_err(|error| QualityError::io(source, error))
  }
}

struct Before {
  snapshot: Snapshot,
  changed_id: Option<EntityId>,
}

fn selector(repo: &str, path: &str, symbol: &str) -> Selector {
  Selector {
    repo: Some(repo.to_owned()),
    path: Some(path.to_owned()),
    symbol: Some(symbol.to_owned()),
    ..Selector::default()
  }
}

fn file_selector(repo: &str, path: &str) -> Selector {
  Selector {
    repo: Some(repo.to_owned()),
    path: Some(path.to_owned()),
    ..Selector::default()
  }
}

fn find(snapshot: &Snapshot, repo: &str, path: &str, symbol: &str) -> Option<EntityId> {
  match bind(snapshot, &selector(repo, path, symbol), None) {
    Binding::One(id) => Some(id),
    _ => None,
  }
}

fn find_file(snapshot: &Snapshot, repo: &str, path: &str) -> Option<EntityId> {
  match bind(snapshot, &file_selector(repo, path), None) {
    Binding::One(id) => Some(id),
    _ => None,
  }
}

fn find_endpoint(snapshot: &Snapshot, method: &str, route: &str) -> Option<EntityId> {
  let endpoint = Selector {
    kind: Some("api_endpoint".to_owned()),
    repo: Some("acme-api".to_owned()),
    method: Some(method.to_owned()),
    route: Some(route.to_owned()),
    ..Selector::default()
  };
  match bind(snapshot, &endpoint, None) {
    Binding::One(id) => Some(id),
    _ => None,
  }
}

fn find_scheme(snapshot: &Snapshot, scheme: &str) -> Option<EntityId> {
  let selector = Selector {
    kind: Some("auth_scheme".to_owned()),
    repo: Some("acme-api".to_owned()),
    scheme: Some(scheme.to_owned()),
    ..Selector::default()
  };
  match bind(snapshot, &selector, None) {
    Binding::One(id) => Some(id),
    _ => None,
  }
}

fn has_edge(snapshot: &Snapshot, kind: EdgeKind, from: Option<EntityId>, to: Option<EntityId>) -> bool {
  match (from, to) {
    (Some(from), Some(to)) => snapshot.edges_between(from, to, &kind).next().is_some(),
    _ => false,
  }
}

fn targeted_by_any(snapshot: &Snapshot, id: EntityId) -> usize {
  snapshot.edges_to(id).filter(|edge| edge.kind != EdgeKind::RenamedFrom).count()
}

fn check(checks: &mut Vec<Check>, name: &str, passed: bool, detail: impl Into<String>) {
  checks.push(Check {
    name: name.to_owned(),
    passed,
    detail: detail.into(),
  });
}

fn lineage_confidence(snapshot: &Snapshot, new: Option<EntityId>, old_key: &str) -> Option<f32> {
  let new = new?;
  snapshot
    .edges_from(new)
    .filter(|edge| edge.kind == EdgeKind::RenamedFrom && edge.attrs.get("old_key").and_then(serde_json::Value::as_str) == Some(old_key))
    .filter_map(|edge| edge.certainty.confidence().map(f32::from))
    .reduce(f32::max)
}

fn scenarios() -> Vec<Scenario> {
  vec![
    Scenario {
      name: "rename_symbol_and_callers",
      description: "rename acme-api formatMoney to formatCents in its definition and both caller files",
      changed: Subject {
        repo: "acme-api",
        path: "src/common/money.ts",
        symbol: "formatMoney",
      },
      edit: |tree| {
        for path in ["src/common/money.ts", "src/orders/orders.service.ts", "src/reports/reports.service.ts"] {
          tree.replace("acme-api", path, "formatMoney", "formatCents")?;
        }
        Ok(())
      },
      deltas: |before, after, checks| {
        let old = before.changed_id;
        let new = find(after, "acme-api", "src/common/money.ts", "formatCents");
        check(
          checks,
          "old symbol key is gone",
          find(after, "acme-api", "src/common/money.ts", "formatMoney").is_none(),
          "formatMoney",
        );
        check(checks, "new symbol key is present", new.is_some(), "formatCents");
        let find_one = find(after, "acme-api", "src/orders/orders.service.ts", "OrdersService.findOne");
        let export_csv = find(after, "acme-api", "src/reports/reports.service.ts", "ReportsService.exportCsv");
        check(
          checks,
          "callers are re-pointed at the new symbol",
          has_edge(after, EdgeKind::Calls, find_one, new) && has_edge(after, EdgeKind::Calls, export_csv, new),
          "OrdersService.findOne and ReportsService.exportCsv call formatCents",
        );
        check(
          checks,
          "no edge still targets the old symbol",
          old.is_none_or(|id| targeted_by_any(after, id) == 0),
          "edges other than renamed_from lineage that target the old entity id",
        );
        let old_key = old.map(|id| before.snapshot.describe(id)).unwrap_or_default();
        let confidence = lineage_confidence(after, new, &old_key);
        check(
          checks,
          "renamed_from lineage links new to old with confidence >= 0.85",
          confidence.is_some_and(|value| value >= MIN_LINEAGE_CONFIDENCE),
          format!("confidence {confidence:?}"),
        );
      },
    },
    Scenario {
      name: "move_file_and_fix_imports",
      description: "move acme-api src/common/money.ts to src/shared/money.ts and update both importers",
      changed: Subject {
        repo: "acme-api",
        path: "src/common/money.ts",
        symbol: "formatMoney",
      },
      edit: |tree| {
        tree.relocate("acme-api", "src/common/money.ts", "src/shared/money.ts")?;
        for path in ["src/orders/orders.service.ts", "src/reports/reports.service.ts"] {
          tree.replace("acme-api", path, "'../common/money'", "'../shared/money'")?;
        }
        Ok(())
      },
      deltas: |before, after, checks| {
        let old = before.changed_id;
        let new = find(after, "acme-api", "src/shared/money.ts", "formatMoney");
        check(
          checks,
          "old file entity is gone",
          find_file(after, "acme-api", "src/common/money.ts").is_none(),
          "src/common/money.ts",
        );
        check(
          checks,
          "new file entity is present",
          find_file(after, "acme-api", "src/shared/money.ts").is_some(),
          "src/shared/money.ts",
        );
        check(checks, "symbol exists under its new key", new.is_some(), "formatMoney at src/shared/money.ts");
        let find_one = find(after, "acme-api", "src/orders/orders.service.ts", "OrdersService.findOne");
        check(
          checks,
          "caller edge is re-pointed",
          has_edge(after, EdgeKind::Calls, find_one, new),
          "OrdersService.findOne -> formatMoney",
        );
        let importer = find_file(after, "acme-api", "src/orders/orders.service.ts");
        let target_file = find_file(after, "acme-api", "src/shared/money.ts");
        check(
          checks,
          "import edge is re-pointed",
          has_edge(after, EdgeKind::Imports, importer, target_file),
          "orders.service.ts imports shared/money.ts",
        );
        check(
          checks,
          "no edge still targets the old symbol",
          old.is_none_or(|id| targeted_by_any(after, id) == 0),
          "edges other than renamed_from lineage that target the old entity id",
        );
        let old_key = old.map(|id| before.snapshot.describe(id)).unwrap_or_default();
        let confidence = lineage_confidence(after, new, &old_key);
        check(
          checks,
          "renamed_from lineage links the moved symbol to its old key",
          confidence.is_some_and(|value| value >= MIN_LINEAGE_CONFIDENCE),
          format!("confidence {confidence:?}"),
        );
      },
    },
    Scenario {
      name: "extract_function",
      description: "extract the fraction parsing of acme-sdk toCents into a new function fractionCents",
      changed: Subject {
        repo: "acme-sdk",
        path: "src/money.ts",
        symbol: "toCents",
      },
      edit: |tree| {
        tree.replace("acme-sdk", "src/money.ts", "Number(fraction.padEnd(2, '0').slice(0, 2));", "fractionCents(fraction);")?;
        let text = tree.read("acme-sdk", "src/money.ts")?;
        tree.write(
          "acme-sdk",
          "src/money.ts",
          &format!("{text}\nfunction fractionCents(fraction: string): number {{\n  return Number(fraction.padEnd(2, '0').slice(0, 2));\n}}\n"),
        )
      },
      deltas: |before, after, checks| {
        let to_cents = find(after, "acme-sdk", "src/money.ts", "toCents");
        let extracted = find(after, "acme-sdk", "src/money.ts", "fractionCents");
        check(checks, "new function entity is present", extracted.is_some(), "fractionCents");
        check(checks, "original symbol key is unchanged", to_cents == before.changed_id && to_cents.is_some(), "toCents");
        check(
          checks,
          "original calls the extracted function",
          has_edge(after, EdgeKind::Calls, to_cents, extracted),
          "toCents -> fractionCents",
        );
        let form = find(after, "acme-web", "src/components/OrderForm.tsx", "OrderForm");
        check(
          checks,
          "callers of the original keep their edge",
          has_edge(after, EdgeKind::Calls, form, to_cents),
          "OrderForm -> toCents",
        );
        let lineage = after.edges.iter().filter(|edge| edge.kind == EdgeKind::RenamedFrom).count();
        check(checks, "no rename is inferred for a pure extraction", lineage == 0, format!("{lineage} renamed_from edges"));
      },
    },
    Scenario {
      name: "delete_function_and_its_calls",
      description: "delete acme-api roundHalfUp and the call and import in ReportsService",
      changed: Subject {
        repo: "acme-api",
        path: "src/billing/rounding.ts",
        symbol: "roundHalfUp",
      },
      edit: |tree| {
        tree.replace("acme-api", "src/reports/reports.service.ts", "import { roundHalfUp } from '../billing/rounding';\n", "")?;
        tree.replace(
          "acme-api",
          "src/reports/reports.service.ts",
          "orders.length ? roundHalfUp(orders.reduce((sum, order) => sum + order.totalCents, 0) / orders.length) : 0",
          "0",
        )?;
        let text = tree.read("acme-api", "src/billing/rounding.ts")?;
        let cut = text
          .find("export function roundHalfUp")
          .ok_or_else(|| QualityError::scenario("delete_function_and_its_calls", "roundHalfUp is not defined where expected"))?;
        tree.write("acme-api", "src/billing/rounding.ts", text[..cut].trim_end())
      },
      deltas: |before, after, checks| {
        let old = before.changed_id;
        check(
          checks,
          "deleted symbol key is gone",
          find(after, "acme-api", "src/billing/rounding.ts", "roundHalfUp").is_none(),
          "roundHalfUp",
        );
        check(
          checks,
          "no edge still targets the deleted symbol",
          old.is_none_or(|id| targeted_by_any(after, id) == 0),
          "edges other than renamed_from lineage that target the old entity id",
        );
        let export_csv = find(after, "acme-api", "src/reports/reports.service.ts", "ReportsService.exportCsv");
        let half_even = find(after, "acme-api", "src/billing/rounding.ts", "roundHalfEven");
        check(checks, "the unrelated sibling survives", half_even.is_some(), "roundHalfEven");
        check(
          checks,
          "the caller no longer references the sibling by mistake",
          !has_edge(after, EdgeKind::Calls, export_csv, half_even),
          "ReportsService.exportCsv -/-> roundHalfEven",
        );
        let lineage = after.edges.iter().filter(|edge| edge.kind == EdgeKind::RenamedFrom).count();
        check(checks, "a deletion is not recorded as a rename", lineage == 0, format!("{lineage} renamed_from edges"));
      },
    },
    Scenario {
      name: "change_route_literal",
      description: "change the UsersController findOne route from ':id' to 'by-id/:id'",
      changed: Subject {
        repo: "acme-api",
        path: "src/users/users.controller.ts",
        symbol: "UsersController.findOne",
      },
      edit: |tree| tree.replace("acme-api", "src/users/users.controller.ts", "@Get(':id')", "@Get('by-id/:id')"),
      deltas: |before, after, checks| {
        let old_endpoint = find_endpoint(&before.snapshot, "GET", "/users/:id");
        check(checks, "old endpoint key is gone", find_endpoint(after, "GET", "/users/:id").is_none(), "GET /users/:id");
        let new_endpoint = find_endpoint(after, "GET", "/users/by-id/:id");
        check(checks, "new endpoint key is present", new_endpoint.is_some(), "GET /users/by-id/:id");
        check(
          checks,
          "the handler edge is re-pointed",
          has_edge(after, EdgeKind::ExposesEndpoint, before.changed_id, new_endpoint),
          "UsersController.findOne exposes the new endpoint",
        );
        check(
          checks,
          "no edge still targets the old endpoint",
          old_endpoint.is_none_or(|id| targeted_by_any(after, id) == 0),
          "edges to the old endpoint id",
        );
        let bearer = find_scheme(after, "bearer");
        check(
          checks,
          "guard-derived auth follows the new endpoint",
          has_edge(after, EdgeKind::SupportsAuth, new_endpoint, bearer),
          "supports_auth bearer on the new endpoint",
        );
        let get_user = find(after, "acme-sdk", "src/client.ts", "AcmeClient.getUser");
        check(
          checks,
          "the SDK is no longer linked to the removed route",
          !has_edge(after, EdgeKind::CallsEndpoint, get_user, old_endpoint),
          "AcmeClient.getUser -/-> GET /users/:id",
        );
      },
    },
    Scenario {
      name: "swap_guard",
      description: "replace JwtAuthGuard by ApiKeyGuard on UsersController",
      changed: Subject {
        repo: "acme-api",
        path: "src/users/users.controller.ts",
        symbol: "UsersController.findOne",
      },
      edit: |tree| {
        tree.replace(
          "acme-api",
          "src/users/users.controller.ts",
          "import { JwtAuthGuard } from '../auth/jwt-auth.guard';",
          "import { ApiKeyGuard } from '../auth/api-key.guard';",
        )?;
        tree.replace(
          "acme-api",
          "src/users/users.controller.ts",
          "@UseGuards(JwtAuthGuard, PermissionsGuard)",
          "@UseGuards(ApiKeyGuard, PermissionsGuard)",
        )
      },
      deltas: |_, after, checks| {
        let endpoint = find_endpoint(after, "GET", "/users/:id");
        let api_key_guard = find(after, "acme-api", "src/auth/api-key.guard.ts", "ApiKeyGuard");
        let jwt_guard = find(after, "acme-api", "src/auth/jwt-auth.guard.ts", "JwtAuthGuard");
        check(
          checks,
          "the endpoint requires the new guard",
          has_edge(after, EdgeKind::RequiresGuard, endpoint, api_key_guard),
          "requires_guard ApiKeyGuard",
        );
        check(
          checks,
          "the endpoint no longer requires the old guard",
          !has_edge(after, EdgeKind::RequiresGuard, endpoint, jwt_guard),
          "requires_guard JwtAuthGuard",
        );
        let api_key = find_scheme(after, "api_key");
        let bearer = find_scheme(after, "bearer");
        check(
          checks,
          "supported schemes follow the guard",
          has_edge(after, EdgeKind::SupportsAuth, endpoint, api_key) && !has_edge(after, EdgeKind::SupportsAuth, endpoint, bearer),
          "api_key yes, bearer no",
        );
      },
    },
  ]
}

pub fn run_refactors(acme: &Path) -> QualityResult<RefactorReport> {
  let mut reports = Vec::new();
  for scenario in scenarios() {
    tracing::info!(scenario = scenario.name, "running refactor scenario");
    reports.push(match run_scenario(&scenario, acme) {
      Ok(report) => report,
      Err(error) => ScenarioReport {
        name: scenario.name.to_owned(),
        description: scenario.description.to_owned(),
        passed: false,
        checks: Vec::new(),
        error: Some(error.to_string()),
      },
    });
  }
  Ok(RefactorReport { scenarios: reports })
}

struct Harness {
  _scratch: TempDir,
  tree: Tree,
  project: Project,
  home: PathBuf,
  engine: Engine,
}

fn project_of(tree: &Path) -> QualityResult<Project> {
  let repos = ACME_REPOS
    .iter()
    .map(|name| -> QualityResult<RepoConfig> {
      Ok(RepoConfig {
        name: RepoName::new(*name).map_err(|error| QualityError::scenario("setup", error.to_string()))?,
        path: tree.join(name),
        include: Vec::new(),
        exclude: Vec::new(),
        languages: None,
      })
    })
    .collect::<QualityResult<Vec<_>>>()?;
  Ok(Project {
    name: ProjectName::new("acme").map_err(|error| QualityError::scenario("setup", error.to_string()))?,
    settings: ProjectSettings::default(),
    repos,
  })
}

fn harness(acme: &Path) -> QualityResult<Harness> {
  let scratch = tempfile::tempdir().map_err(|error| QualityError::io(std::env::temp_dir(), error))?;
  let tree = scratch.path().join("tree");
  for repo in ACME_REPOS {
    copy_tree(&acme.join(repo), &tree.join(repo))?;
  }
  let project = project_of(&tree)?;
  let home = scratch.path().join("home");
  let engine = Engine::open(&project, &AtlasHome::rooted_at(&home), EngineOptions::default())?;
  engine.ensure_fresh()?;
  Ok(Harness {
    _scratch: scratch,
    tree: Tree { root: tree },
    project,
    home,
    engine,
  })
}

fn inferred_fact(subject: EntityId, repo: &str, path: &str, hash: ContentHash) -> QualityResult<NewInferredFact> {
  let rel = RelPath::new(path).map_err(|error| QualityError::scenario("inferred", error.to_string()))?;
  let confidence = Confidence::new(0.8).map_err(|error| QualityError::scenario("inferred", error.to_string()))?;
  let repo = RepoName::new(repo).map_err(|error| QualityError::scenario("inferred", error.to_string()))?;
  Ok(NewInferredFact {
    subjects: vec![subject],
    relation: "equivalent_to".to_owned(),
    payload: serde_json::json!({ "origin": "atlas-eval quality probe" }),
    certainty: Certainty::Inferred {
      confidence,
      provider: "atlas-eval".to_owned(),
      model: "probe".to_owned(),
      rationale: "synthetic fact used to test staleness".to_owned(),
    },
    evidence: vec![Evidence {
      repo,
      path: rel,
      span: Span::from_zero_based_rows(0, 1, 0, 0, 0, 1),
      content_hash: hash,
      producer: Producer::new("atlas-eval", 1),
      note: None,
    }],
    created_run: None,
    llm_call: None,
  })
}

fn file_hash(engine: &Engine, repo: &str, path: &str) -> QualityResult<ContentHash> {
  let (repo, path) = (
    RepoName::new(repo).map_err(|error| QualityError::scenario("hash", error.to_string()))?,
    RelPath::new(path).map_err(|error| QualityError::scenario("hash", error.to_string()))?,
  );
  engine
    .store()
    .read(|tx| tx.file(&repo, &path))?
    .map(|record| record.content_hash)
    .ok_or_else(|| QualityError::scenario("hash", format!("{repo}:{path} is not indexed")))
}

fn run_scenario(scenario: &Scenario, acme: &Path) -> QualityResult<ScenarioReport> {
  let harness = harness(acme)?;
  let before = Snapshot::load(&harness.engine, &harness.project)?;
  let changed_id = find(&before, scenario.changed.repo, scenario.changed.path, scenario.changed.symbol);
  let control_subject = find(&before, "acme-billing", "src/money.rs", "to_cents");
  let (changed_fact, control_fact) = match (changed_id, control_subject) {
    (Some(changed), Some(control)) => {
      let changed_hash = file_hash(&harness.engine, scenario.changed.repo, scenario.changed.path)?;
      let control_hash = file_hash(&harness.engine, "acme-billing", "src/money.rs")?;
      let changed_fact = inferred_fact(changed, scenario.changed.repo, scenario.changed.path, changed_hash)?;
      let control_fact = inferred_fact(control, "acme-billing", "src/money.rs", control_hash)?;
      harness
        .engine
        .store()
        .write(|tx| -> Result<_, atlas_store::StoreError> { Ok((tx.insert_inferred_fact(&changed_fact)?, tx.insert_inferred_fact(&control_fact)?)) })?;
      (Some(changed), Some(control))
    }
    _ => (None, None),
  };
  (scenario.edit)(&harness.tree)?;
  let report = harness.engine.ensure_fresh()?;
  let mut checks = Vec::new();
  check(
    &mut checks,
    "incremental update ran",
    report.header().run.is_some() && report.is_fresh(),
    format!("{} files updated", report.header().files_updated),
  );
  let after = Snapshot::load(&harness.engine, &harness.project)?;
  let before = Before { snapshot: before, changed_id };
  (scenario.deltas)(&before, &after, &mut checks);
  check_equivalence(&harness, &after, &mut checks)?;
  check_no_stale_facts(&harness, &after, &mut checks)?;
  check_inferred(&harness, changed_fact.is_some() && control_fact.is_some(), changed_id, control_subject, &mut checks)?;
  Ok(ScenarioReport {
    name: scenario.name.to_owned(),
    description: scenario.description.to_owned(),
    passed: checks.iter().all(|check| check.passed),
    checks,
    error: None,
  })
}

fn check_equivalence(harness: &Harness, incremental: &Snapshot, checks: &mut Vec<Check>) -> QualityResult<()> {
  let reference_home = harness.home.with_file_name("reference-home");
  let reference = Engine::open(&harness.project, &AtlasHome::rooted_at(&reference_home), EngineOptions::default())?;
  reference.ensure_fresh()?;
  let lineage = Owner::Project { pass: PASS_LINEAGE.to_owned() };
  let facts = harness.engine.store().read(|tx| tx.owned_facts(&lineage))?;
  reference.store().write(|tx| tx.replace_owner(&lineage, &facts))?;
  let incremental_digest = harness.engine.store().read(|tx| tx.fact_set_digest())?;
  let full_digest = reference.store().read(|tx| tx.fact_set_digest())?;
  let detail = match incremental_digest == full_digest {
    true => format!("digest {incremental_digest}"),
    false => {
      let full = Snapshot::load(&reference, &harness.project)?;
      format!("incremental {incremental_digest} != full {full_digest}; {}", difference(incremental, &full))
    }
  };
  check(checks, "incremental index equals a full index (digest)", incremental_digest == full_digest, detail);
  Ok(())
}

fn difference(incremental: &Snapshot, full: &Snapshot) -> String {
  let entities: BTreeSet<String> = incremental
    .entities
    .keys()
    .chain(full.entities.keys())
    .filter(|id| incremental.entities.contains_key(id) != full.entities.contains_key(id))
    .map(|id| id.to_string())
    .collect();
  let edges = |snapshot: &Snapshot| -> BTreeSet<String> {
    snapshot
      .edges
      .iter()
      .map(|edge| format!("{} {} -> {} {}", edge.kind, snapshot.describe(edge.from), snapshot.describe(edge.to), edge.certainty.tag()))
      .collect()
  };
  let (left, right) = (edges(incremental), edges(full));
  let only_incremental: Vec<&String> = left.difference(&right).take(3).collect();
  let only_full: Vec<&String> = right.difference(&left).take(3).collect();
  format!("{} entities differ; edges only incremental {only_incremental:?}; only full {only_full:?}", entities.len())
}

fn check_no_stale_facts(harness: &Harness, snapshot: &Snapshot, checks: &mut Vec<Check>) -> QualityResult<()> {
  let mut stale = BTreeSet::new();
  for edge in snapshot.edges.iter().filter(|edge| !edge.certainty.is_llm_derived()) {
    for evidence in &edge.evidence {
      if evidence_is_stale(harness, evidence) {
        stale.insert(format!(
          "{} {} -> {} ({}:{})",
          edge.kind,
          snapshot.describe(edge.from),
          snapshot.describe(edge.to),
          evidence.repo,
          evidence.path
        ));
      }
    }
  }
  let missing_files: Vec<String> = snapshot
    .entities
    .values()
    .filter_map(|entity| Some((entity.repo.as_ref()?, entity.path.as_ref()?)))
    .filter(|(repo, path)| !harness.tree.path(repo.as_str(), path.as_str()).exists())
    .map(|(repo, path)| format!("{repo}:{path}"))
    .collect::<BTreeSet<_>>()
    .into_iter()
    .collect();
  let detail = match (stale.is_empty(), missing_files.is_empty()) {
    (true, true) => "every deterministic edge cites current file content and every located entity's file exists".to_owned(),
    _ => format!(
      "stale edges: {:?}; entities in missing files: {:?}",
      stale.iter().take(5).collect::<Vec<_>>(),
      missing_files.iter().take(5).collect::<Vec<_>>()
    ),
  };
  check(checks, "no stale deterministic fact survives", stale.is_empty() && missing_files.is_empty(), detail);
  Ok(())
}

fn evidence_is_stale(harness: &Harness, evidence: &Evidence) -> bool {
  match fs::read(harness.tree.path(evidence.repo.as_str(), evidence.path.as_str())) {
    Ok(bytes) => ContentHash::of(&bytes) != evidence.content_hash,
    Err(_) => true,
  }
}

fn check_inferred(harness: &Harness, seeded: bool, changed: Option<EntityId>, control: Option<EntityId>, checks: &mut Vec<Check>) -> QualityResult<()> {
  if !seeded {
    check(
      checks,
      "inferred facts on changed evidence are stale",
      false,
      "the probe facts could not be seeded because the subject symbols are not indexed",
    );
    return Ok(());
  }
  let facts = harness.engine.store().read(|tx| {
    tx.inferred_facts(&InferredQuery {
      subject: None,
      relation: None,
      freshness: FreshnessPolicy::IncludeStale,
    })
  })?;
  let current_on_changed = facts
    .iter()
    .filter(|fact| !fact.subjects.iter().any(|subject| Some(*subject) == control))
    .filter(|fact| fact.freshness == Freshness::Current)
    .count();
  let control_current = facts
    .iter()
    .any(|fact| fact.subjects.iter().any(|subject| Some(*subject) == control) && fact.freshness == Freshness::Current);
  let describe = facts.iter().map(|fact| format!("{:?}", fact.freshness)).collect::<Vec<_>>().join(", ");
  check(
    checks,
    "inferred facts on changed evidence are stale",
    current_on_changed == 0,
    format!("{current_on_changed} fact(s) on changed evidence still current; freshness of all facts: [{describe}]; changed subject {changed:?}"),
  );
  check(
    checks,
    "inferred facts on unchanged evidence stay current",
    control_current,
    "control fact on acme-billing money.rs",
  );
  Ok(())
}
