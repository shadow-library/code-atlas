mod common;

use atlas_query::{Caveat, ImpactRequest, Targeted, TraverseRequest, UnusedRequest};

fn found<T: std::fmt::Debug>(targeted: Targeted<T>) -> T {
  match targeted {
    Targeted::Found(answer) => answer,
    other => panic!("expected a resolved target, got {other:?}"),
  }
}

fn dynamic_total(caveats: &[Caveat]) -> Option<usize> {
  caveats.iter().find_map(|caveat| match caveat {
    Caveat::DynamicMemberCalls { total, .. } => Some(*total),
    _ => None,
  })
}

#[test]
fn should_report_dynamic_member_calls_for_callers_of_a_namespace_member() {
  let fixture = common::fixture("adversarial", &["dynamic-ts"]);

  let response = fixture.query.callers(&TraverseRequest::new("stop")).unwrap();
  let text = fixture.text(&response);

  let answer = found(response.answer);
  assert_eq!(dynamic_total(&answer.caveats), Some(2));
  assert!(text.contains("2 dynamic member calls could reach this (computed property access)"), "{text}");
  assert!(text.contains("dynamic-ts:src/dispatch.ts:5 table.[methodName]"), "{text}");
  assert!(text.contains("dynamic-ts:src/dispatch.ts:9 target.[methodName]"), "{text}");
}

#[test]
fn should_count_dynamic_member_calls_as_unresolved_risk_in_impact() {
  let fixture = common::fixture("adversarial", &["dynamic-ts"]);

  let answer = found(fixture.query.impact(&ImpactRequest::new(["stop"])).unwrap().answer);

  assert_eq!(dynamic_total(&answer.unresolved_risk), Some(2));
}

#[test]
fn should_not_report_dynamic_member_calls_from_unrelated_files() {
  let fixture = common::single_repo(
    "app",
    &[
      ("package.json", "{\n  \"name\": \"app\",\n  \"private\": true\n}\n"),
      ("src/target.ts", "export function stop(): string {\n  return 'stop';\n}\n"),
      (
        "src/other.ts",
        "export function other(table: Record<string, () => string>, key: string): string {\n  return table[key]();\n}\n",
      ),
    ],
  );

  let answer = found(fixture.query.callers(&TraverseRequest::new("stop")).unwrap().answer);

  assert_eq!(dynamic_total(&answer.caveats), None);
}

#[test]
fn should_treat_a_target_with_dynamic_member_calls_in_its_importers_as_possibly_unused() {
  let fixture = common::fixture("adversarial", &["dynamic-ts"]);

  let answer = fixture
    .query
    .unused(&UnusedRequest {
      limit: 100,
      ..UnusedRequest::default()
    })
    .unwrap()
    .answer;

  let stop = answer
    .likely_unused
    .items
    .iter()
    .chain(&answer.possibly_unused.items)
    .find(|candidate| candidate.entity.short == "stop")
    .expect("stop reported");
  assert_eq!(stop.class, atlas_query::UnusedClass::PossiblyUnused);
  assert!(
    stop
      .evidence
      .iter()
      .any(|line| line == "2 dynamic member calls could reach this (computed property access)"),
    "{stop:#?}"
  );
}
