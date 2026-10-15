mod common;

use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::{Detection, EdgeKind};
use atlas_detect::{DeclaredRoutesDetector, DecoratorFamily, NestEndpointDetector};

use common::{Analyzed, analyze, node, render, render_source, run, sorted};

fn corpus() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/family-corpus")
}

fn detector() -> DeclaredRoutesDetector {
  let text = fs::read_to_string(corpus().join("acme-server.toml")).unwrap();
  let parsed = DecoratorFamily::parse("acme-server", &text).unwrap();
  assert_eq!(parsed.warnings, Vec::<String>::new());
  DeclaredRoutesDetector::new(vec![parsed.family])
}

fn file(name: &str) -> Analyzed {
  let source = fs::read_to_string(corpus().join("projects/src").join(name)).unwrap();
  analyze("projects", &format!("src/{name}"), &source)
}

fn detect(name: &str) -> (Analyzed, Detection, Vec<String>) {
  let analyzed = file(name);
  let (detection, diagnostics) = run(&detector(), &analyzed);
  let diagnostics = diagnostics.iter().map(|diagnostic| format!("{} {}", diagnostic.code, diagnostic.message)).collect();
  (analyzed, detection, diagnostics)
}

fn edges_with_attrs(analyzed: &Analyzed, detection: &Detection, kinds: &[EdgeKind]) -> Vec<String> {
  sorted(
    detection
      .edges
      .iter()
      .filter(|edge| kinds.contains(&edge.kind))
      .map(|edge| {
        format!(
          "{} {} -> {} {} {}",
          edge.kind,
          node(&analyzed.facts, &edge.from),
          node(&analyzed.facts, &edge.to),
          edge.certainty.tag(),
          serde_json::to_string(&edge.attrs).unwrap()
        )
      })
      .collect(),
  )
}

fn entity_attrs(detection: &Detection, prefix: &str) -> Vec<String> {
  sorted(
    detection
      .entities
      .iter()
      .filter(|entity| entity.key.to_string().starts_with(prefix))
      .map(|entity| format!("{} {} {}", entity.key, entity.certainty.tag(), serde_json::to_string(&entity.attrs).unwrap()))
      .collect(),
  )
}

#[test]
fn should_make_a_guard_decorator_without_a_guard_class_argument_the_guard_itself() {
  let (analyzed, detection, _) = detect("projects.controller.ts");
  let guards: Vec<String> = sorted(render(&analyzed.facts, &detection))
    .into_iter()
    .filter(|line| line.starts_with("requires_guard") || line.starts_with("implements_auth") || line.contains("guard_decorator:") || line.starts_with("auth:"))
    .collect();
  assert_eq!(
    guards,
    [
      "auth:projects:bearer [syntactic]",
      "implements_auth other:guard_decorator:projects:acme-server:Auth -> auth:projects:bearer [syntactic]",
      "implements_auth other:guard_decorator:projects:acme-server:Authenticated -> auth:projects:bearer [syntactic]",
      "other:guard_decorator:projects:acme-server:Auth [syntactic]",
      "other:guard_decorator:projects:acme-server:Authenticated [syntactic]",
      "other:guard_decorator:projects:acme-server:Guarded [syntactic]",
      "requires_guard endpoint:projects:DELETE /projects/{id} -> ?SessionGuard [syntactic]",
      "requires_guard endpoint:projects:DELETE /projects/{id} -> other:guard_decorator:projects:acme-server:Authenticated [syntactic]",
      "requires_guard endpoint:projects:DELETE /projects/{id} -> other:guard_decorator:projects:acme-server:Guarded [syntactic]",
      "requires_guard endpoint:projects:GET /projects -> other:guard_decorator:projects:acme-server:Authenticated [syntactic]",
      "requires_guard endpoint:projects:GET /projects/bot -> other:guard_decorator:projects:acme-server:Auth [syntactic]",
      "requires_guard endpoint:projects:GET /projects/bot -> other:guard_decorator:projects:acme-server:Authenticated [syntactic]",
      "requires_guard endpoint:projects:POST /projects -> other:guard_decorator:projects:acme-server:Auth [syntactic]",
      "requires_guard endpoint:projects:POST /projects -> other:guard_decorator:projects:acme-server:Authenticated [syntactic]",
    ]
  );
  assert_eq!(
    entity_attrs(&detection, "other:guard_decorator:"),
    [
      r#"other:guard_decorator:projects:acme-server:Auth [syntactic] {"auth_scheme":"bearer","decorator":"Auth","framework":"acme-server"}"#,
      r#"other:guard_decorator:projects:acme-server:Authenticated [syntactic] {"auth_scheme":"bearer","decorator":"Authenticated","framework":"acme-server"}"#,
      r#"other:guard_decorator:projects:acme-server:Guarded [syntactic] {"decorator":"Guarded","framework":"acme-server"}"#,
    ]
  );
}

#[test]
fn should_record_guard_options_and_arguments_and_never_target_a_same_file_value() {
  let (analyzed, detection, _) = detect("projects.controller.ts");
  assert_eq!(
    edges_with_attrs(&analyzed, &detection, &[EdgeKind::RequiresGuard]),
    [
      r#"requires_guard endpoint:projects:DELETE /projects/{id} -> ?SessionGuard [syntactic] {"level":"method","order":2}"#,
      r#"requires_guard endpoint:projects:DELETE /projects/{id} -> other:guard_decorator:projects:acme-server:Authenticated [syntactic] {"level":"class","order":0}"#,
      r#"requires_guard endpoint:projects:DELETE /projects/{id} -> other:guard_decorator:projects:acme-server:Guarded [syntactic] {"arguments":["projects:read"],"level":"method","order":1}"#,
      r#"requires_guard endpoint:projects:GET /projects -> other:guard_decorator:projects:acme-server:Authenticated [syntactic] {"level":"class","order":0}"#,
      r#"requires_guard endpoint:projects:GET /projects/bot -> other:guard_decorator:projects:acme-server:Auth [syntactic] {"arguments":["admin",{"binding":"Scope.Admin"},3],"level":"method","order":1}"#,
      r#"requires_guard endpoint:projects:GET /projects/bot -> other:guard_decorator:projects:acme-server:Authenticated [syntactic] {"level":"class","order":0}"#,
      r#"requires_guard endpoint:projects:POST /projects -> other:guard_decorator:projects:acme-server:Auth [syntactic] {"level":"method","options":{"optional":true,"roles":["owner","admin"],"scope":{"binding":"Scope.Admin"}},"order":1}"#,
      r#"requires_guard endpoint:projects:POST /projects -> other:guard_decorator:projects:acme-server:Authenticated [syntactic] {"level":"class","order":0}"#,
    ]
  );
}

#[test]
fn should_resolve_permission_constants_of_the_same_file_and_name_unread_ones() {
  let (analyzed, detection, diagnostics) = detect("projects.controller.ts");
  assert_eq!(
    edges_with_attrs(&analyzed, &detection, &[EdgeKind::RequiresPermission, EdgeKind::References]),
    [
      r#"references permission:projects:PROJECTS_WRITE -> ?PROJECTS_WRITE [syntactic] {"permission_constant":true}"#,
      r#"references permission:projects:Scope.Admin -> ?Scope::Admin [syntactic] {"permission_constant":true}"#,
      r#"requires_permission ProjectsController.bot -> permission:projects:bot:read [resolved] {"decorator":"BotPermission","level":"method"}"#,
      r#"requires_permission ProjectsController.create -> permission:projects:PROJECTS_WRITE [heuristic 0.60] {"constant":"PROJECTS_WRITE","decorator":"Permit","level":"method"}"#,
      r#"requires_permission ProjectsController.create -> permission:projects:projects:audit [syntactic] {"decorator":"Permit","level":"method"}"#,
      r#"requires_permission ProjectsController.list -> permission:projects:projects:read [resolved] {"constant":"PROJECTS_READ","decorator":"Permit","level":"method"}"#,
      r#"requires_permission ProjectsController.remove -> permission:projects:Scope.Admin [heuristic 0.60] {"constant":"Scope.Admin","decorator":"BotPermission","level":"method"}"#,
    ]
  );
  assert_eq!(
    entity_attrs(&detection, "permission:"),
    [
      r#"permission:projects:PROJECTS_WRITE [heuristic 0.60] {"constant":"PROJECTS_WRITE","decorator":"Permit","value_source":"unread_constant"}"#,
      r#"permission:projects:Scope.Admin [heuristic 0.60] {"constant":"Scope.Admin","decorator":"BotPermission","value_source":"unread_constant"}"#,
      r#"permission:projects:bot:read [resolved] {"decorator":"BotPermission","value_source":"constant"}"#,
      r#"permission:projects:projects:audit [syntactic] {"decorator":"Permit","value_source":"literal"}"#,
      r#"permission:projects:projects:read [resolved] {"constant":"PROJECTS_READ","decorator":"Permit","value_source":"constant"}"#,
    ]
  );
  assert_eq!(diagnostics, ["permission-not-literal Permit on ProjectsController.bot has a non-literal argument"]);
}

#[test]
fn should_match_aliased_and_namespace_guard_decorators_only_when_imported_from_the_family() {
  let (analyzed, detection, _) = detect("aliases.controller.ts");
  assert_eq!(
    edges_with_attrs(&analyzed, &detection, &[EdgeKind::RequiresGuard]),
    [
      r#"requires_guard endpoint:projects:GET /aliases -> other:guard_decorator:projects:acme-server:Auth [syntactic] {"level":"method","options":{"roles":["viewer"]},"order":1}"#,
      r#"requires_guard endpoint:projects:GET /aliases -> other:guard_decorator:projects:acme-server:Authenticated [syntactic] {"level":"class","order":0}"#,
    ]
  );
}

#[test]
fn should_keep_nest_guards_and_permissions_unchanged_for_argument_free_and_constant_shapes() {
  let source = "import { Controller, Get, UseGuards } from '@nestjs/common';\nimport { ADMIN, RequirePermission } from './auth';\n\n@Controller('nest')\n@UseGuards()\nexport class NestController {\n  @Get()\n  @UseGuards(ADMIN)\n  @RequirePermission(ADMIN)\n  list() {\n    return [];\n  }\n}\n";
  assert_eq!(
    render_source(&NestEndpointDetector, "nest", "src/nest.controller.ts", source),
    [
      "endpoint:nest:GET /nest [syntactic]",
      "exposes_endpoint NestController.list -> endpoint:nest:GET /nest [syntactic]",
      "requires_guard endpoint:nest:GET /nest -> ?ADMIN [syntactic]",
    ]
  );
}

#[test]
fn should_take_only_class_shaped_arguments_of_a_guard_decorator_as_guard_classes_and_never_same_file_values() {
  let source = "import { Api, Guarded, Read } from '@acme/server';\nimport * as guards from './guards';\nimport { AdminGuard, AuthGuard, Role } from './guards';\n\nconst LocalGuard = AuthGuard('local');\nconst Admin = Role.Admin;\nconst Frozen = Object.freeze({ admin: 'admin' });\nconst Label = String(Role.Admin);\nconst Roles = new Map();\nconst sessionGuard = new AdminGuard();\n\n@Api('shapes')\nexport class ShapesController {\n  @Read('a')\n  @Guarded(LocalGuard, guards.SessionGuard, new AdminGuard())\n  a() {\n    return null;\n  }\n\n  @Read('b')\n  @Guarded(Admin, sessionGuard, Role.Admin, Frozen, Label, Roles)\n  b() {\n    return null;\n  }\n}\n";
  let analyzed = analyze("shapes", "src/shapes.controller.ts", source);
  let (detection, _) = run(&detector(), &analyzed);
  assert_eq!(
    edges_with_attrs(&analyzed, &detection, &[EdgeKind::RequiresGuard]),
    [
      r#"requires_guard endpoint:shapes:GET /shapes/a -> ?AdminGuard [syntactic] {"level":"method","order":2}"#,
      r#"requires_guard endpoint:shapes:GET /shapes/a -> ?LocalGuard [syntactic] {"level":"method","order":0}"#,
      r#"requires_guard endpoint:shapes:GET /shapes/a -> ?guards::SessionGuard [syntactic] {"level":"method","order":1}"#,
      r#"requires_guard endpoint:shapes:GET /shapes/b -> other:guard_decorator:shapes:acme-server:Guarded [syntactic] {"arguments":[{"binding":"Admin"},{"binding":"sessionGuard"},{"binding":"Role.Admin"},{"binding":"Frozen"},{"binding":"Label"},{"binding":"Roles"}],"level":"method","order":0}"#,
    ]
  );
}

#[test]
fn should_key_an_imported_permission_constant_by_its_exported_name_whatever_the_import_style() {
  let (analyzed, detection, _) = detect("imports.controller.ts");
  assert_eq!(
    edges_with_attrs(&analyzed, &detection, &[EdgeKind::RequiresPermission, EdgeKind::References]),
    [
      r#"references permission:projects:PROJECTS_WRITE -> ?P::PROJECTS_WRITE [syntactic] {"permission_constant":true}"#,
      r#"references permission:projects:PROJECTS_WRITE -> ?PW [syntactic] {"permission_constant":true}"#,
      r#"references permission:projects:Scope.Admin -> ?P::Scope::Admin [syntactic] {"permission_constant":true}"#,
      r#"references permission:projects:Scope.Admin -> ?Scope::Admin [syntactic] {"permission_constant":true}"#,
      r#"requires_permission ImportsController.aliased -> permission:projects:PROJECTS_WRITE [heuristic 0.60] {"constant":"PROJECTS_WRITE","decorator":"Permit","level":"method"}"#,
      r#"requires_permission ImportsController.member -> permission:projects:Scope.Admin [heuristic 0.60] {"constant":"Scope.Admin","decorator":"Permit","level":"method"}"#,
      r#"requires_permission ImportsController.namespaced -> permission:projects:PROJECTS_WRITE [heuristic 0.60] {"constant":"PROJECTS_WRITE","decorator":"Permit","level":"method"}"#,
      r#"requires_permission ImportsController.namespaced -> permission:projects:Scope.Admin [heuristic 0.60] {"constant":"Scope.Admin","decorator":"Permit","level":"method"}"#,
    ]
  );
}
