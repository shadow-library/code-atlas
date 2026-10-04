mod common;

use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use atlas_engine::FaultInjection;
use common::{ACME_REPOS, start_configured, text_of};
use rmcp::model::CallToolRequestParams;
use serde_json::{Value, json};

const CALLS: usize = 8;
const RENDEZVOUS: Duration = Duration::from_secs(20);

#[tokio::test]
async fn should_overlap_concurrent_tool_calls_and_share_barrier_scans() {
  let entered = Arc::new(AtomicUsize::new(0));
  let peak = Arc::new(AtomicUsize::new(0));
  let hook = {
    let (entered, peak) = (Arc::clone(&entered), Arc::clone(&peak));
    move |tool: &str| {
      if tool != "atlas_search" {
        return;
      }
      let arrived = entered.fetch_add(1, Ordering::SeqCst) + 1;
      let started = Instant::now();
      while entered.load(Ordering::SeqCst) < CALLS && started.elapsed() < RENDEZVOUS {
        thread::sleep(Duration::from_millis(1));
      }
      peak.fetch_max(arrived.max(entered.load(Ordering::SeqCst)), Ordering::SeqCst);
    }
  };
  let harness = start_configured(&ACME_REPOS, hook, |options| {
    options.engine.fault_injection = FaultInjection::before_index_lock(|| thread::sleep(Duration::from_millis(300)));
  })
  .await;
  harness.text("atlas_status", json!({})).await;
  let money = common::repo_path(harness.scratch.path(), "acme-billing").join("src/money.rs");
  let mut source = fs::read_to_string(&money).unwrap();
  source.push_str("\npub fn zz_overlap_marker_fn() -> i64 {\n  9\n}\n");
  fs::write(&money, source).unwrap();
  let scans = harness.server.engine().barrier_scans();

  let calls: Vec<_> = (0..CALLS)
    .map(|_| {
      let peer = harness.client.peer().clone();
      let Value::Object(arguments) = json!({ "query": "zz_overlap_marker_fn" }) else {
        unreachable!()
      };
      tokio::spawn(async move { peer.call_tool(CallToolRequestParams::new("atlas_search").with_arguments(arguments)).await.unwrap() })
    })
    .collect();
  let mut results = Vec::new();
  for call in calls {
    results.push(call.await.unwrap());
  }

  assert_eq!(peak.load(Ordering::SeqCst), CALLS, "every call must be in flight at the same time");
  let shared = harness.server.engine().barrier_scans() - scans;
  assert!(shared <= 2, "{CALLS} overlapping calls must share barrier scans, but {shared} scans ran");
  for result in &results {
    let text = text_of(result);
    assert_ne!(result.is_error, Some(true), "{text}");
    assert!(text.starts_with("fresh run="), "{text}");
    assert!(text.contains("zz_overlap_marker_fn"), "an edit made before the calls must be visible: {text}");
  }
  harness.stop().await.unwrap();
}
