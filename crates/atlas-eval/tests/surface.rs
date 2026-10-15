#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use atlas_eval::agent::{
  ClaudeConfig, ConfigIsolation, DEFAULT_JUDGE_MODEL, DeltaRange, McpArm, SURFACE_DEFAULT_MODEL, SessionLedger, SurfaceOptions, TokenSection, ToolLoading, extend_surface,
  render_surface_markdown, run_surface,
};

const FAKE_ATLAS: &str = r#"#!/bin/sh
case "$1" in
  --version) echo "catl 0.0.0-fake"; exit 0 ;;
  project|index) exit 0 ;;
  mcp)
    printf '%s\n' "$*" > "$(dirname "$0")/mcp-args"
    read -r line
    printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/message","params":{}}'
    printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"fake","version":"1"},"instructions":"Hello"}}'
    read -r line
    read -r line
    printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"atlas_a","description":"dd","inputSchema":{"type":"object"}}]}}'
    read -r line
    ;;
esac
"#;

fn fake_claude(counter: &Path, honours_tool_search: bool) -> String {
  format!(
    r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "9.9.9 (Claude Code)"; exit 0; fi
echo x >> "{}"
mcp=""
tools_arg=""
while [ $# -gt 0 ]; do
  case "$1" in
    --mcp-config) mcp="$2"; shift 2 ;;
    --tools) tools_arg="$2"; shift 2 ;;
    *) shift ;;
  esac
done
create=100; atlas=0; tools='["Bash"]'; servers='[]'
if grep -q code-atlas "$mcp"; then
  servers='[{{"name":"code-atlas","status":"connected"}}]'
  if [ "$ENABLE_TOOL_SEARCH" = "true" ] && [ "{}" = "yes" ] && echo "$tools_arg" | grep -q ToolSearch; then atlas=20; tools='["Bash","ToolSearch"]'; else atlas=300; tools='["Bash","mcp__code-atlas__atlas_a"]'; fi
fi
total=$((create + atlas))
printf '%s\n' "{{\"type\":\"system\",\"subtype\":\"init\",\"tools\":$tools,\"mcp_servers\":$servers}}"
printf '%s\n' "{{\"type\":\"assistant\",\"message\":{{\"id\":\"m1\",\"content\":[{{\"type\":\"text\",\"text\":\"ok\"}}],\"usage\":{{\"input_tokens\":3,\"output_tokens\":2,\"cache_read_input_tokens\":0,\"cache_creation_input_tokens\":$total}}}}}}"
printf '%s\n' "{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"num_turns\":1,\"result\":\"ok\",\"usage\":{{\"input_tokens\":3,\"output_tokens\":2,\"cache_read_input_tokens\":0,\"cache_creation_input_tokens\":$total}}}}"
"#,
    counter.display(),
    if honours_tool_search { "yes" } else { "no" }
  )
}

const FAILING_CLAUDE: &str = "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo '9.9.9'; exit 0; fi\necho 'Not logged in' >&2\nexit 1\n";

fn script(dir: &Path, name: &str, text: &str) -> PathBuf {
  let path = dir.join(name);
  fs::write(&path, text).unwrap();
  fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
  path
}

fn options(root: &Path, claude: PathBuf, sessions: bool) -> SurfaceOptions {
  let fixture = root.join("fixture-src");
  fs::create_dir_all(fixture.join("repo-a")).unwrap();
  fs::write(fixture.join("repo-a/a.ts"), "export const a = 1;\n").unwrap();
  SurfaceOptions {
    atlas_bin: script(root, "catl", FAKE_ATLAS),
    claude: ClaudeConfig {
      binary: claude,
      model: SURFACE_DEFAULT_MODEL.to_owned(),
      judge_model: DEFAULT_JUDGE_MODEL.to_owned(),
      config_isolation: ConfigIsolation::User,
    },
    fixture,
    work_dir: root.join("work"),
    sessions,
    tools: None,
  }
}

#[test]
fn should_measure_bytes_and_per_loading_deltas_with_twelve_sessions() {
  let root = tempfile::tempdir().unwrap();
  let counter = root.path().join("sessions.count");
  let claude = script(root.path(), "claude", &fake_claude(&counter, true));
  let report = run_surface(&options(root.path(), claude, true)).unwrap();
  assert_eq!(report.bytes.instructions_bytes, 5);
  assert_eq!(report.bytes.tool_count, 1);
  assert_eq!(report.bytes.tools[0].name, "atlas_a");
  assert_eq!(report.claude_code_version.as_deref(), Some("9.9.9 (Claude Code)"));
  assert_eq!(report.atlas_version.as_deref(), Some("catl 0.0.0-fake"));
  assert_eq!(fs::read_to_string(&counter).unwrap().lines().count(), 12);
  let TokenSection::Measured(measurement) = &report.tokens else {
    panic!("tokens were not measured")
  };
  assert_eq!((measurement.sessions_started, measurement.retries_used), (12, 0));
  assert_eq!(measurement.configurations.len(), 4);
  assert!(measurement.configurations.iter().all(|result| result.samples.len() == 3));
  let delta = |loading: ToolLoading| measurement.deltas.iter().find(|delta| delta.loading == loading).unwrap().prompt_total;
  assert_eq!(delta(ToolLoading::Upfront), DeltaRange { median: 300, low: 300, high: 300 });
  assert_eq!(delta(ToolLoading::SearchToolDenied), DeltaRange { median: 300, low: 300, high: 300 });
  let deferred_atlas = measurement
    .configurations
    .iter()
    .find(|result| result.arm == McpArm::CodeAtlas && result.loading == ToolLoading::SearchToolDenied)
    .unwrap();
  assert!(!deferred_atlas.samples[0].exposure.tool_search_tool_present);
  assert_eq!(deferred_atlas.samples[0].exposure.atlas_tool_count, 1);
  let markdown = render_surface_markdown(&report);
  assert!(markdown.contains("| Code Atlas, tool search on, `ToolSearch` not allowed (`ENABLE_TOOL_SEARCH=true`) | 3 |"));
  assert!(!markdown.contains(root.path().to_str().unwrap()));
}

#[test]
fn should_stop_after_one_session_when_claude_is_not_logged_in() {
  let root = tempfile::tempdir().unwrap();
  let claude = script(root.path(), "claude", FAILING_CLAUDE);
  let report = run_surface(&options(root.path(), claude, true)).unwrap();
  let TokenSection::NotMeasured { reason } = &report.tokens else {
    panic!("tokens were measured")
  };
  assert!(reason.starts_with("the first session failed: process did not exit cleanly"), "{reason}");
  assert_eq!(report.bytes.tool_count, 1);
}

#[test]
fn should_mark_tokens_not_measured_without_a_claude_binary() {
  let root = tempfile::tempdir().unwrap();
  let report = run_surface(&options(root.path(), root.path().join("missing-claude"), true)).unwrap();
  assert_eq!(
    report.tokens,
    TokenSection::NotMeasured {
      reason: "the claude binary is unavailable".to_owned()
    }
  );
}

#[test]
fn should_start_the_server_with_the_requested_tool_profile_and_record_it() {
  let root = tempfile::tempdir().unwrap();
  let mut options = options(root.path(), root.path().join("missing-claude"), false);
  options.tools = Some("core".to_owned());

  let report = run_surface(&options).unwrap();

  assert_eq!(fs::read_to_string(root.path().join("mcp-args")).unwrap(), "mcp --project surface --tools core\n");
  assert_eq!(report.tools.as_deref(), Some("core"));
  assert!(render_surface_markdown(&report).contains("\n- tools: core\n"));
}

fn extension_base(root: &Path, claude: &Path) -> atlas_eval::agent::SurfaceReport {
  let report = run_surface(&options(root, claude.to_owned(), true)).unwrap();
  let json = serde_json::to_string(&report).unwrap();
  serde_json::from_str(&json).unwrap()
}

#[test]
fn should_merge_the_tool_search_allowed_mode_into_a_recorded_report_with_six_sessions() {
  let root = tempfile::tempdir().unwrap();
  let counter = root.path().join("sessions.count");
  let claude = script(root.path(), "claude", &fake_claude(&counter, true));
  let recorded = extension_base(root.path(), &claude);
  fs::remove_file(&counter).unwrap();
  let report = extend_surface(&options(root.path(), claude, true), recorded).unwrap();
  assert_eq!(fs::read_to_string(&counter).unwrap().lines().count(), 6);
  let TokenSection::Measured(measurement) = &report.tokens else {
    panic!("tokens were not measured")
  };
  assert_eq!(measurement.configurations.len(), 6);
  assert_eq!(
    measurement.extension,
    Some(SessionLedger {
      sessions_started: 6,
      retries_used: 0
    })
  );
  assert_eq!(measurement.extension_note, None);
  let delta = measurement.deltas.iter().find(|delta| delta.loading == ToolLoading::SearchAllowed).unwrap();
  assert_eq!(delta.prompt_total, DeltaRange { median: 20, low: 20, high: 20 });
  let allowed = measurement
    .configurations
    .iter()
    .find(|result| result.arm == McpArm::CodeAtlas && result.loading == ToolLoading::SearchAllowed)
    .unwrap();
  assert_eq!(allowed.samples[0].exposure.tools, vec!["Bash", "ToolSearch"]);
  assert!(render_surface_markdown(&report).contains("tool search on, `ToolSearch` allowed"));
}

#[test]
fn should_stop_the_extension_after_one_session_when_atlas_tools_are_still_listed() {
  let root = tempfile::tempdir().unwrap();
  let counter = root.path().join("sessions.count");
  let claude = script(root.path(), "claude", &fake_claude(&counter, false));
  let recorded = extension_base(root.path(), &claude);
  fs::remove_file(&counter).unwrap();
  let report = extend_surface(&options(root.path(), claude, true), recorded).unwrap();
  assert_eq!(fs::read_to_string(&counter).unwrap().lines().count(), 1);
  let TokenSection::Measured(measurement) = &report.tokens else {
    panic!("tokens were not measured")
  };
  assert_eq!(
    measurement.extension,
    Some(SessionLedger {
      sessions_started: 1,
      retries_used: 0
    })
  );
  assert!(measurement.extension_note.as_deref().unwrap().contains("not run"));
  assert!(measurement.deltas.iter().all(|delta| delta.loading != ToolLoading::SearchAllowed));
}

#[test]
fn should_resume_an_extension_without_repeating_recorded_sessions() {
  let root = tempfile::tempdir().unwrap();
  let counter = root.path().join("sessions.count");
  let claude = script(root.path(), "claude", &fake_claude(&counter, true));
  let recorded = extension_base(root.path(), &claude);
  let mut partial = extend_surface(&options(root.path(), claude.clone(), true), recorded).unwrap();
  let TokenSection::Measured(measurement) = &mut partial.tokens else {
    panic!("tokens were not measured")
  };
  for result in measurement.configurations.iter_mut().filter(|result| result.loading == ToolLoading::SearchAllowed) {
    result.samples.truncate(if result.arm == McpArm::CodeAtlas { 1 } else { 0 });
  }
  measurement
    .configurations
    .retain(|result| result.loading != ToolLoading::SearchAllowed || !result.samples.is_empty());
  measurement.extension = Some(SessionLedger {
    sessions_started: 1,
    retries_used: 0,
  });
  fs::remove_file(&counter).unwrap();
  let report = extend_surface(&options(root.path(), claude, true), partial).unwrap();
  assert_eq!(fs::read_to_string(&counter).unwrap().lines().count(), 5);
  let TokenSection::Measured(measurement) = &report.tokens else {
    panic!("tokens were not measured")
  };
  assert_eq!(
    measurement.extension,
    Some(SessionLedger {
      sessions_started: 6,
      retries_used: 0
    })
  );
  assert!(
    measurement
      .configurations
      .iter()
      .filter(|result| result.loading == ToolLoading::SearchAllowed)
      .all(|result| result.samples.len() == 3)
  );
}
