use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use atlas_core::{ContentHash, RunId};
use atlas_lsp::protocol::{Location, ReferenceContext, ReferenceParams, TextDocumentIdentifier, TextDocumentPositionParams};
use atlas_lsp::{
  ChangeKind, ChangeList, FileChange, Launch, Lease, LineBreaks, LineIndex, LspPool, PoolOptions, ServerCommand, ServerState, ServerUnit, SyncDemand, TypeScriptSettings,
  UnitFolder, UnitSettings, canonical_path, path_from_uri, uri_from_path,
};

fn runs(command: &ServerCommand) -> bool {
  let mut probe = Command::new(&command.program);
  probe.args(command.args.iter().filter(|arg| *arg != "--stdio")).arg("--version");
  probe.output().is_ok_and(|output| output.status.success())
}

fn command_for(entry: PathBuf) -> ServerCommand {
  match entry.extension().and_then(|extension| extension.to_str()) {
    Some("mjs" | "js") => ServerCommand::new("node").arg(entry).arg("--stdio"),
    _ => ServerCommand::new(entry).arg("--stdio"),
  }
}

fn npx_cache_entries() -> Vec<PathBuf> {
  let Some(home) = env::var_os("HOME") else {
    return Vec::new();
  };
  let Ok(caches) = fs::read_dir(Path::new(&home).join(".npm/_npx")) else {
    return Vec::new();
  };
  caches
    .flatten()
    .map(|cache| cache.path().join("node_modules/typescript-language-server/lib/cli.mjs"))
    .filter(|entry| entry.is_file())
    .collect()
}

fn locate_typescript_language_server() -> Result<ServerCommand, String> {
  let configured = env::var_os("ATLAS_TEST_TS_LSP").map(PathBuf::from);
  let on_path = env::var_os("PATH").and_then(|paths| {
    env::split_paths(&paths)
      .map(|directory| directory.join("typescript-language-server"))
      .find(|candidate| candidate.is_file())
  });
  configured
    .into_iter()
    .chain(on_path)
    .chain(npx_cache_entries())
    .map(command_for)
    .find(runs)
    .ok_or_else(|| "set ATLAS_TEST_TS_LSP, put typescript-language-server on PATH, or cache it with npx; tests never download it".to_owned())
}

fn references(lease: &Lease, file: &Path, text: &str, name: &str) -> Vec<(PathBuf, u32)> {
  let offset = text.find(name).unwrap();
  let position = LineIndex::new(text, lease.line_breaks()).position(offset, lease.encoding()).unwrap();
  let locations: Option<Vec<Location>> = lease
    .request_with_timeout(
      "textDocument/references",
      ReferenceParams {
        position: TextDocumentPositionParams {
          text_document: TextDocumentIdentifier {
            uri: uri_from_path(file).unwrap(),
          },
          position,
        },
        context: ReferenceContext { include_declaration: false },
      },
      Duration::from_secs(120),
    )
    .unwrap();
  let mut found: Vec<(PathBuf, u32)> = locations
    .unwrap_or_default()
    .into_iter()
    .map(|location| (canonical_path(&path_from_uri(&location.uri).unwrap()).unwrap(), location.range.start.line))
    .collect();
  found.sort();
  found
}

#[test]
fn should_answer_references_with_typescript_language_server_and_see_synced_edits() {
  let command = match locate_typescript_language_server() {
    Ok(command) => command,
    Err(reason) => {
      eprintln!("SKIPPED should_answer_references_with_typescript_language_server_and_see_synced_edits: {reason}");
      return;
    }
  };
  let work = tempfile::tempdir().unwrap();
  let root = canonical_path(&work.path().join("shop")).unwrap();
  fs::create_dir_all(root.join("src")).unwrap();
  fs::write(
    root.join("tsconfig.json"),
    r#"{"compilerOptions": {"strict": true, "module": "commonjs", "target": "es2020"}, "include": ["src"]}"#,
  )
  .unwrap();
  let money = root.join("src/money.ts");
  let usage = root.join("src/use.ts");
  let money_text = "export function toCents(amount: number): number {\n  return Math.round(amount * 100);\n}\n";
  fs::write(&money, money_text).unwrap();
  fs::write(&usage, "import { toCents } from './money';\n\nexport const price = toCents(1.5);\n").unwrap();
  let unit = ServerUnit::new(
    "shop",
    vec![UnitFolder::new("shop", &root)],
    UnitSettings::TypeScript(TypeScriptSettings::default()),
    command.cwd(&root),
    Launch::Installed,
  );
  let pool = LspPool::new(PoolOptions::one_shot());

  let synced = pool
    .sync(&unit, Some(RunId(1)), SyncDemand::Best, |_| unreachable!(), std::slice::from_ref(&money))
    .unwrap();

  assert_eq!(synced.lease.state(), ServerState::Ready);
  assert_eq!(synced.lease.line_breaks(), LineBreaks::Ecma);
  assert!(synced.lease.typescript_version().is_some_and(|version| version.version.starts_with('5')));
  assert_eq!(synced.report.opened, vec![money.clone()]);
  assert_eq!(references(&synced.lease, &money, money_text, "toCents"), vec![(usage.clone(), 0), (usage.clone(), 2)]);
  drop(synced);

  fs::write(
    &usage,
    "import { toCents } from './money';\n\nexport const price = toCents(1.5);\nexport const total = toCents(2) + toCents(3);\n",
  )
  .unwrap();
  let hash = ContentHash::of(&fs::read(&usage).unwrap());
  let synced = pool
    .sync(
      &unit,
      Some(RunId(2)),
      SyncDemand::Best,
      |_| ChangeList::Known(vec![FileChange::new(&usage, ChangeKind::Changed(hash))]),
      std::slice::from_ref(&money),
    )
    .unwrap();

  assert_eq!(synced.report.opened, vec![usage.clone()]);
  assert_eq!(
    references(&synced.lease, &money, money_text, "toCents"),
    vec![(usage.clone(), 0), (usage.clone(), 2), (usage.clone(), 3), (usage.clone(), 3)]
  );
}
