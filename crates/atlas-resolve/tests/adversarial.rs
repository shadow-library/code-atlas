mod common;

use atlas_core::{CertaintyClass, EdgeKind, EntityKind, RefKind, UnresolvedReason};
use common::{Project, file, rust, ts};

const RESOLVED: &[CertaintyClass] = &[CertaintyClass::Resolved];
const HEURISTIC: &[CertaintyClass] = &[CertaintyClass::Heuristic];
const STRUCTURAL: &[CertaintyClass] = &[CertaintyClass::Syntactic, CertaintyClass::Resolved];
const AUTHORITATIVE: &[CertaintyClass] = &[CertaintyClass::Resolved, CertaintyClass::CompilerVerified, CertaintyClass::Syntactic];

fn macro_generated() -> UnresolvedReason {
  UnresolvedReason::Unsupported {
    detail: "macro_generated".to_owned(),
  }
}

mod trait_dispatch {
  use super::*;

  const REPO: &str = "trait-dispatch-rust";

  fn item(items: &[&str], kind: EntityKind) -> atlas_core::EntityId {
    rust(REPO, REPO, &[], items, kind)
  }

  #[test]
  fn should_resolve_trait_receivers_to_the_trait_method_with_heuristic_impl_candidates() {
    let project = Project::adversarial(REPO);
    let cases = [("read_dyn", "get"), ("read_impl", "get"), ("read_generic", "get"), ("write_boxed", "put")];
    for (caller, method) in cases {
      let from = item(&[caller], EntityKind::Function);
      let declaration = project.assert_edge(EdgeKind::Calls, from, item(&["Store", method], EntityKind::Method), RESOLVED);
      assert_eq!(declaration.certainty, atlas_core::Certainty::Resolved, "{caller}");
      for implementor in ["MemStore", "DiskStore"] {
        let concrete = item(&[implementor, "<Store>", method], EntityKind::Method);
        let edge = project.assert_edge(EdgeKind::Calls, from, concrete, HEURISTIC);
        assert_eq!(
          edge.certainty,
          atlas_core::Certainty::heuristic(atlas_core::Confidence::new(0.3).unwrap(), "dyn-dispatch-candidate")
        );
        project.assert_no_edge_of_class(EdgeKind::Calls, from, concrete, AUTHORITATIVE);
      }
    }
  }

  #[test]
  fn should_resolve_concrete_receivers_and_self_calls_to_the_exact_impl() {
    let project = Project::adversarial(REPO);
    let read_mem = item(&["read_mem"], EntityKind::Function);
    project.assert_edge(EdgeKind::Calls, read_mem, item(&["MemStore", "<Store>", "get"], EntityKind::Method), RESOLVED);
    project.assert_no_edge(EdgeKind::Calls, read_mem, item(&["DiskStore", "<Store>", "get"], EntityKind::Method));
    project.assert_no_edge(EdgeKind::Calls, read_mem, item(&["Store", "get"], EntityKind::Method));
    let path_for = item(&["DiskStore", "path_for"], EntityKind::Method);
    project.assert_edge(EdgeKind::Calls, item(&["DiskStore", "<Store>", "get"], EntityKind::Method), path_for, RESOLVED);
    project.assert_no_edge(EdgeKind::Calls, item(&["MemStore", "<Store>", "get"], EntityKind::Method), path_for);
  }

  #[test]
  fn should_derive_implements_edges_from_impl_blocks() {
    let project = Project::adversarial(REPO);
    let store = item(&["Store"], EntityKind::Trait);
    for implementor in ["MemStore", "DiskStore"] {
      project.assert_edge(EdgeKind::Implements, item(&[implementor], EntityKind::Struct), store, STRUCTURAL);
      for method in ["get", "put"] {
        project.assert_edge(
          EdgeKind::Implements,
          item(&[implementor, "<Store>", method], EntityKind::Method),
          item(&["Store", method], EntityKind::Method),
          RESOLVED,
        );
      }
    }
  }
}

mod duplicate_names {
  use super::*;

  const REPO: &str = "duplicate-names-rust";

  fn parse_in(module: &str) -> atlas_core::EntityId {
    rust(REPO, REPO, &[module], &["parse"], EntityKind::Function)
  }

  #[test]
  fn should_follow_explicit_imports_and_module_paths() {
    let project = Project::adversarial(REPO);
    let split_fields = rust(REPO, REPO, &["consumer"], &["split_fields"], EntityKind::Function);
    project.assert_edge(EdgeKind::Calls, split_fields, parse_in("a"), RESOLVED);
    project.assert_no_edge(EdgeKind::Calls, split_fields, parse_in("b"));
    let read_number = rust(REPO, REPO, &["consumer"], &["read_number"], EntityKind::Function);
    project.assert_edge(EdgeKind::Calls, read_number, parse_in("b"), RESOLVED);
    project.assert_no_edge(EdgeKind::Calls, read_number, parse_in("a"));
  }

  #[test]
  fn should_let_an_explicit_import_shadow_a_glob() {
    let project = Project::adversarial(REPO);
    let explicit = rust(REPO, REPO, &["consumer", "glob_explicit"], &["explicit_beats_glob"], EntityKind::Function);
    project.assert_edge(EdgeKind::Calls, explicit, parse_in("b"), RESOLVED);
    project.assert_no_edge(EdgeKind::Calls, explicit, parse_in("a"));
  }

  #[test]
  fn should_report_two_competing_globs_as_ambiguous_without_edges() {
    let project = Project::adversarial(REPO);
    let ambiguous = rust(REPO, REPO, &["ambiguous"], &["ambiguous_call"], EntityKind::Function);
    let mut candidates = vec![parse_in("a"), parse_in("b")];
    candidates.sort();
    project.assert_unresolved(ambiguous, "parse", &UnresolvedReason::Ambiguous { candidates });
    project.assert_no_edge(EdgeKind::Calls, ambiguous, parse_in("a"));
    project.assert_no_edge(EdgeKind::Calls, ambiguous, parse_in("b"));
  }

  #[test]
  fn should_index_cfg_gated_modules_and_record_the_cfg() {
    let project = Project::adversarial(REPO);
    let placement = project.result(REPO, "src/ambiguous.rs");
    assert_eq!(
      placement.reachability,
      atlas_resolve::Reachability::CfgGated {
        cfg: "feature = \"ambiguous-demo\"".to_owned()
      }
    );
    assert_eq!(project.result(REPO, "src/a.rs").reachability, atlas_resolve::Reachability::Reachable);
  }
}

mod macro_heavy {
  use super::*;

  const REPO: &str = "macro-heavy-rust";

  fn item(items: &[&str], kind: EntityKind) -> atlas_core::EntityId {
    rust(REPO, REPO, &[], items, kind)
  }

  #[test]
  fn should_resolve_macro_invocations_to_the_preceding_macro_rules() {
    let project = Project::adversarial(REPO);
    project.assert_edge(EdgeKind::References, file(REPO, "src/lib.rs"), item(&["define_ops"], EntityKind::Macro), RESOLVED);
    project.assert_edge(EdgeKind::References, file(REPO, "src/lib.rs"), item(&["getter"], EntityKind::Macro), RESOLVED);
  }

  #[test]
  fn should_never_resolve_macro_generated_functions() {
    let project = Project::adversarial(REPO);
    let manhattan = item(&["Point", "manhattan"], EntityKind::Method);
    let plain = item(&["plain"], EntityKind::Function);
    for (from, name) in [(manhattan, "add"), (manhattan, "x"), (manhattan, "y"), (plain, "sub")] {
      project.assert_unresolved(from, name, &macro_generated());
    }
    assert!(project.edges_from(EdgeKind::Calls, manhattan).is_empty());
    project.assert_edge(EdgeKind::Calls, plain, item(&["helper"], EntityKind::Function), RESOLVED);
    project.assert_no_edge(EdgeKind::Calls, plain, manhattan);
  }
}

mod shadowing {
  use super::*;

  const REPO: &str = "shadowing-ts";

  fn render(name: &str) -> atlas_core::EntityId {
    ts(REPO, REPO, &["src", "render"], &[name], EntityKind::Function)
  }

  #[test]
  fn should_bind_calls_by_scope_not_by_name() {
    let project = Project::adversarial(REPO);
    let format = ts(REPO, REPO, &["src", "format"], &["format"], EntityKind::Function);
    project.assert_edge(EdgeKind::Calls, render("renderImported"), format, RESOLVED);
    let block_scoped = project.assert_edge(EdgeKind::Calls, render("renderBlockScoped"), format, RESOLVED);
    assert_eq!(block_scoped.evidence.len(), 1);
    for shadowed in ["renderLocal", "renderParam"] {
      project.assert_no_edge(EdgeKind::Calls, render(shadowed), format);
      project.assert_no_edge(EdgeKind::References, render(shadowed), format);
    }
  }
}

mod reexport_chain {
  use super::*;

  const REPO: &str = "reexport-chain-ts";

  #[test]
  fn should_follow_aliases_and_star_exports_to_the_original_definition() {
    let project = Project::adversarial(REPO);
    let compute_total = ts(REPO, REPO, &["src", "core", "impl"], &["computeTotal"], EntityKind::Function);
    let checkout = ts(REPO, REPO, &["src", "app", "checkout"], &["checkout"], EntityKind::Function);
    project.assert_edge(EdgeKind::Calls, checkout, compute_total, RESOLVED);
    project.assert_no_edge(EdgeKind::Calls, checkout, ts(REPO, REPO, &["src", "other", "total"], &["total"], EntityKind::Function));
    project.assert_edge(EdgeKind::ReExports, file(REPO, "src/core/index.ts"), compute_total, STRUCTURAL);
    project.assert_edge(EdgeKind::ReExports, file(REPO, "src/lib/index.ts"), compute_total, STRUCTURAL);
    project.assert_edge(EdgeKind::ReExports, file(REPO, "src/index.ts"), file(REPO, "src/lib/index.ts"), STRUCTURAL);
  }
}

mod dynamic {
  use super::*;

  const REPO: &str = "dynamic-ts";

  fn dispatch(name: &str) -> atlas_core::EntityId {
    ts(REPO, REPO, &["src", "dispatch"], &[name], EntityKind::Function)
  }

  #[test]
  fn should_leave_dynamic_constructs_unresolved_without_fabricated_edges() {
    let project = Project::adversarial(REPO);
    let start = ts(REPO, REPO, &["src", "handlers"], &["start"], EntityKind::Function);
    let stop = ts(REPO, REPO, &["src", "handlers"], &["stop"], EntityKind::Function);
    for caller in ["invoke", "invokeOn"] {
      project.assert_unresolved(dispatch(caller), "[methodName]", &UnresolvedReason::Dynamic);
      project.assert_no_edge(EdgeKind::Calls, dispatch(caller), start);
      project.assert_no_edge(EdgeKind::Calls, dispatch(caller), stop);
    }
    project.assert_unresolved(dispatch("loadHandler"), "modulePath", &UnresolvedReason::Dynamic);
    project.assert_unresolved(dispatch("loadPlugin"), "name", &UnresolvedReason::Dynamic);
    project.assert_unresolved(dispatch("runExpression"), "eval", &UnresolvedReason::Dynamic);
    for caller in ["loadHandler", "loadPlugin", "runExpression"] {
      assert!(project.edges_from(EdgeKind::Calls, dispatch(caller)).is_empty(), "{caller} must not call anything");
    }
    project.assert_no_edge(EdgeKind::Imports, dispatch("loadPlugin"), file(REPO, "src/plugins/alpha.ts"));
    project.assert_no_edge(EdgeKind::Imports, file(REPO, "src/dispatch.ts"), file(REPO, "src/plugins/alpha.ts"));
    let dynamic_import = project.unresolved_named(dispatch("loadPlugin"), "name");
    assert_eq!(dynamic_import[0].kind, RefKind::Call);
    assert_eq!(dynamic_import[0].path, ["import", "name"]);
  }

  #[test]
  fn should_resolve_the_static_namespace_call() {
    let project = Project::adversarial(REPO);
    project.assert_edge(
      EdgeKind::Calls,
      dispatch("staticCall"),
      ts(REPO, REPO, &["src", "handlers"], &["start"], EntityKind::Function),
      RESOLVED,
    );
  }
}
