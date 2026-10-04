mod common;

use common::{ACME_REPOS, start};
use serde_json::{Value, json};

#[tokio::test]
async fn should_advertise_dependents_and_dependencies_like_the_call_traversal_tools() {
  let harness = start(&ACME_REPOS).await;

  let tools = harness.client.list_all_tools().await.unwrap();

  for name in ["atlas_dependents", "atlas_dependencies"] {
    let tool = tools.iter().find(|tool| tool.name == name).unwrap_or_else(|| panic!("{name} is not advertised"));
    let callers = tools.iter().find(|tool| tool.name == "atlas_callers").unwrap();
    assert_eq!(tool.input_schema.get("required"), callers.input_schema.get("required"), "{name}");
    let mut properties: Vec<&String> = tool.input_schema["properties"].as_object().unwrap().keys().collect();
    let mut expected: Vec<&String> = callers.input_schema["properties"].as_object().unwrap().keys().collect();
    properties.sort_unstable();
    expected.sort_unstable();
    assert_eq!(properties, expected, "{name}");
    let description = tool.description.as_deref().unwrap();
    assert!(description.contains("[heuristic]/[inferred]") && description.contains("verify"), "{name}: {description}");
    assert_eq!(tool.annotations.as_ref().and_then(|annotations| annotations.read_only_hint), Some(true), "{name}");
  }
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_list_what_depends_on_a_class_with_atlas_dependents() {
  let harness = start(&ACME_REPOS).await;

  let text = harness.text("atlas_dependents", json!({ "target": "UsersService" })).await;

  assert!(text.starts_with("fresh run="), "{text}");
  assert!(text.contains("dependents of UsersService"), "{text}");
  for expected in ["src/users/users.controller.ts", "src/users/users.module.ts", "src/users/users.service.spec.ts", "UsersController.usersService", "UsersModule"] {
    assert!(text.contains(expected), "missing {expected}: {text}");
  }
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_list_what_a_class_depends_on_with_atlas_dependencies() {
  let harness = start(&ACME_REPOS).await;

  let text = harness.text("atlas_dependencies", json!({ "target": "UsersController", "depth": 1 })).await;

  assert!(text.contains("dependencies of UsersController"), "{text}");
  assert!(text.contains("JwtAuthGuard") && text.contains("PermissionsGuard"), "{text}");
  let document: Value = serde_json::from_str(&harness.text("atlas_dependencies", json!({ "target": "UsersController", "format": "json" })).await).unwrap();
  assert_eq!(document["freshness"]["state"], "fresh");
  assert_eq!(document["answer"]["outcome"], "found");
  assert_eq!(document["answer"]["kind"], "dependencies");
  harness.stop().await.unwrap();
}

#[tokio::test]
async fn should_validate_dependency_tool_arguments_like_atlas_callers() {
  let harness = start(&ACME_REPOS).await;

  for name in ["atlas_dependents", "atlas_dependencies"] {
    assert!(harness.failure(name, json!({ "target": "UsersService", "depth": 99 })).await.contains("depth must be between 1 and 6"), "{name}");
    assert!(harness.failure(name, json!({ "target": "  " })).await.contains("target must not be empty"), "{name}");
    assert!(harness.text(name, json!({ "target": "NoSuchThingAnywhere" })).await.contains("not found"), "{name}");
  }
  harness.stop().await.unwrap();
}
