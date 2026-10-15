mod common;

use atlas_detect::DecoratorFamily;
use atlas_project::ProjectSettings;
use atlas_query::ApiRequest;

const FAMILY: &str = "packages = [\"@acme/gate\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nRead = \"GET\"\n[guards]\ndecorators = [\"Authenticated\"]\nauth_schemes = { Authenticated = \"bearer\" }\n[access]\npublic = [\"Public\"]\noptional = [\"OptionalAuth\"]\n";
const PACKAGE: &str = "{ \"name\": \"gate\", \"private\": true, \"dependencies\": { \"@acme/gate\": \"^1.0.0\" } }\n";
const CONTROLLER: &str = "import { Api, Authenticated, OptionalAuth, Public, Read } from '@acme/gate';\n\n@Api('feed')\n@Authenticated()\nexport class FeedController {\n  @Read()\n  @OptionalAuth()\n  list() {\n    return [];\n  }\n}\n\n@Api('docs')\n@Public()\nexport class DocsController {\n  @Read('drafts')\n  @Authenticated()\n  drafts() {\n    return [];\n  }\n}\n";

#[test]
fn should_mark_optional_guards_and_show_a_contested_class_marker_with_a_heuristic_requirement() {
  let mut settings = ProjectSettings::default();
  settings.detect.route_families = vec!["acme-gate".to_owned()];
  settings.detect.families = vec![DecoratorFamily::parse("acme-gate", FAMILY).unwrap().family];
  let fixture = common::single_repo_configured("gate", &[("package.json", PACKAGE), ("src/controllers.ts", CONTROLLER)], settings, common::engine_options());

  let response = fixture.query.api(&ApiRequest::default()).unwrap();
  let text = fixture.text(&response);

  let expected = [
    "  GET /docs/drafts gate:src/controllers.ts:16 [syntactic]",
    "    handler DocsController.drafts ([syntactic])",
    "    guard acme-gate:Authenticated ([syntactic]), implements auth bearer ([syntactic])",
    "    possibly supports auth bearer ([heuristic 0.99]) (all_of [heuristic 0.99, link])",
    r#"    auth auth_combinator=all_of [heuristic 0.50, link], auth_marker={"applied":false,"contested_by":"method","decorator":"Public","level":"class","mode":"public"} [syntactic, file], auth_requirement={"all_of":["bearer"]} [heuristic 0.50, link]"#,
    "    consumers: unknown (no client->endpoint links in index)",
    "  GET /feed gate:src/controllers.ts:6 [syntactic]",
    "    handler FeedController.list ([syntactic])",
    "    guard acme-gate:Authenticated ([syntactic]) (optional), implements auth bearer ([syntactic])",
    "    possibly supports auth bearer ([heuristic 0.99]) (optional [heuristic 0.99, link])",
    r#"    auth auth_marker={"decorator":"OptionalAuth","level":"method"} [syntactic, file]"#,
  ]
  .join("\n");
  assert!(text.contains(&expected), "{text}");
}

const BLEND_FAMILY: &str = "packages = [\"@acme/blend\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nRead = \"GET\"\n[guards]\ndecorators = [\"Authenticated\"]\nauth_schemes = { Authenticated = [\"bearer\", \"session_cookie\"] }\n[guards.semantics]\ndefault = { class_marker = \"method_wins\" }\n[permissions]\ndecorators = [\"RequireScope\"]\nauth_schemes = { RequireScope = [\"bearer\", \"session_cookie\"] }\n[access]\npublic = [\"Public\"]\n";
const BLEND_PACKAGE: &str = "{ \"name\": \"blend\", \"private\": true, \"dependencies\": { \"@acme/blend\": \"^1.0.0\" } }\n";
const BLEND_CONTROLLER: &str = "import { Api, Authenticated, Public, Read, RequireScope } from '@acme/blend';\n\n@Api('vault')\n@Authenticated()\n@RequireScope('vault:read')\nexport class VaultController {\n  @Read()\n  list() {\n    return [];\n  }\n}\n\n@Api('catalog')\n@Public()\nexport class CatalogController {\n  @Read('drafts')\n  @Authenticated()\n  drafts() {\n    return [];\n  }\n}\n";

#[test]
fn should_show_one_any_of_group_for_identical_scheme_groups_and_an_overridden_class_marker_without_a_cap() {
  let mut settings = ProjectSettings::default();
  settings.detect.route_families = vec!["acme-blend".to_owned()];
  settings.detect.families = vec![DecoratorFamily::parse("acme-blend", BLEND_FAMILY).unwrap().family];
  let fixture = common::single_repo_configured(
    "blend",
    &[("package.json", BLEND_PACKAGE), ("src/controllers.ts", BLEND_CONTROLLER)],
    settings,
    common::engine_options(),
  );

  let response = fixture.query.api(&ApiRequest::default()).unwrap();
  let text = fixture.text(&response);

  let expected = [
    "  GET /catalog/drafts blend:src/controllers.ts:16 [syntactic]",
    "    handler CatalogController.drafts ([syntactic])",
    "    guard acme-blend:Authenticated ([syntactic]), implements auth session_cookie ([syntactic]), implements auth bearer ([syntactic])",
    "    possibly supports auth bearer ([heuristic 0.99]) (any_of [heuristic 0.99, link])",
    "    possibly supports auth session_cookie ([heuristic 0.99]) (any_of [heuristic 0.99, link])",
    r#"    auth auth_marker={"applied":false,"decorator":"Public","level":"class","mode":"public","overridden_by":"method"} [syntactic, file]"#,
    "    consumers: unknown (no client->endpoint links in index)",
    "  GET /vault blend:src/controllers.ts:7 [syntactic]",
    "    handler VaultController.list ([syntactic])",
    "    guard acme-blend:Authenticated ([syntactic]), implements auth session_cookie ([syntactic]), implements auth bearer ([syntactic])",
    "    guard acme-blend:RequireScope ([syntactic]), implements auth session_cookie ([syntactic]), implements auth bearer ([syntactic])",
    "    possibly supports auth session_cookie ([heuristic 0.99]) (any_of [heuristic 0.99, link])",
    "    possibly supports auth bearer ([heuristic 0.99]) (any_of [heuristic 0.99, link])",
  ]
  .join("\n");
  assert!(text.contains(&expected), "{text}");
}
