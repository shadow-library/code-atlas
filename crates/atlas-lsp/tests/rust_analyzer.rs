use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use atlas_core::{ContentHash, RunId, Span};
use atlas_lsp::protocol::{
  ClientCapabilities, DidOpenTextDocumentParams, DocumentSymbolParams, DocumentSymbolResponse, InitializeParams, Location, ReferenceContext, ReferenceParams,
  TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams, WorkspaceFolder,
};
use atlas_lsp::{
  ChangeKind, ChangeList, Client, ClientOptions, FileChange, Launch, Lease, LineBreaks, LineIndex, LspPool, NoopHandler, PoolOptions, PositionEncoding, ProcessExit,
  RustAnalyzerSettings, ServerCommand, ServerState, ServerUnit, ShutdownOutcome, SyncDemand, UnitFolder, UnitSettings, canonical_path, path_from_uri, uri_from_path,
};
use serde_json::json;

fn locate_rust_analyzer() -> Result<PathBuf, String> {
  let on_path = env::var_os("PATH").and_then(|paths| {
    env::split_paths(&paths)
      .map(|directory| directory.join("rust-analyzer"))
      .find(|candidate| candidate.is_file())
  });
  let from_rustup = Command::new("rustup")
    .args(["which", "rust-analyzer"])
    .output()
    .ok()
    .filter(|output| output.status.success())
    .map(|output| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()));
  [on_path, from_rustup]
    .into_iter()
    .flatten()
    .find(|binary| Command::new(binary).arg("--version").output().is_ok_and(|output| output.status.success()))
    .ok_or_else(|| "rust-analyzer is not on PATH and `rustup which rust-analyzer` found nothing that runs".to_owned())
}

#[test]
fn should_initialize_and_shut_down_real_rust_analyzer() {
  let binary = match locate_rust_analyzer() {
    Ok(binary) => binary,
    Err(reason) => {
      eprintln!("SKIPPED should_initialize_and_shut_down_real_rust_analyzer: rust-analyzer unavailable ({reason})");
      return;
    }
  };
  let work = tempfile::tempdir().unwrap();
  let root = canonical_path(&work.path().join("tiny crate")).unwrap();
  fs::create_dir_all(root.join("src")).unwrap();
  fs::write(root.join("Cargo.toml"), "[package]\nname = \"tiny\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
  let source = "// é\rcarriage\npub fn answer() -> u32 { 42 }\n";
  fs::write(root.join("src/lib.rs"), source).unwrap();
  let root_uri = uri_from_path(&root).unwrap();
  let command = ServerCommand::new(binary).cwd(&root).env("CARGO_TARGET_DIR", work.path().join("target"));
  let client = Client::spawn(&command, Arc::new(NoopHandler), ClientOptions::default()).unwrap();
  let params = InitializeParams {
    process_id: Some(std::process::id()),
    root_uri: Some(root_uri.clone()),
    capabilities: ClientCapabilities::standard(vec![PositionEncoding::Utf8, PositionEncoding::Utf16]),
    initialization_options: Some(json!({
      "cargo": {"buildScripts": {"enable": false}},
      "procMacro": {"enable": false},
      "checkOnSave": false,
      "cachePriming": {"enable": false}
    })),
    workspace_folders: Some(vec![WorkspaceFolder {
      uri: root_uri,
      name: "tiny".to_owned(),
    }]),
    ..InitializeParams::default()
  };

  let initialized = client.initialize(&params).unwrap();

  assert_eq!(initialized.encoding, PositionEncoding::Utf8);
  assert_eq!(initialized.result.server_info.map(|info| info.name).as_deref(), Some("rust-analyzer"));
  assert!(initialized.result.capabilities.supports_prepare_rename());
  assert!(initialized.result.capabilities.references_provider.is_supported());
  let uri = uri_from_path(&root.join("src/lib.rs")).unwrap();
  client
    .notify(
      "textDocument/didOpen",
      DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
          uri: uri.clone(),
          language_id: "rust".to_owned(),
          version: 1,
          text: source.to_owned(),
        },
      },
    )
    .unwrap();
  let symbols: Option<DocumentSymbolResponse> = client
    .request_with_timeout(
      "textDocument/documentSymbol",
      DocumentSymbolParams {
        text_document: TextDocumentIdentifier { uri },
      },
      Duration::from_secs(120),
    )
    .unwrap();
  let Some(DocumentSymbolResponse::Nested(symbols)) = symbols else {
    panic!("expected hierarchical symbols, got {symbols:?}");
  };
  let answer = symbols.iter().find(|symbol| symbol.name == "answer").unwrap();
  let span = LineIndex::new(source, LineBreaks::Lf).span(answer.selection_range, initialized.encoding).unwrap();
  let start = source.find("answer").unwrap();
  assert_eq!(span, Span::of_text(source, start, start + "answer".len()).unwrap());
  assert_eq!((span.start_line, span.start_col), (2, 7));
  assert_eq!(client.shutdown(), ShutdownOutcome::Exited(ProcessExit::Code(0)));
}

fn references(lease: &Lease, file: &std::path::Path, text: &str, name: &str) -> Vec<(PathBuf, u32)> {
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
    .map(|location| (path_from_uri(&location.uri).unwrap(), location.range.start.line))
    .collect();
  found.sort();
  found
}

#[test]
fn should_answer_references_through_the_pool_and_see_synced_edits() {
  let binary = match locate_rust_analyzer() {
    Ok(binary) => binary,
    Err(reason) => {
      eprintln!("SKIPPED should_answer_references_through_the_pool_and_see_synced_edits: rust-analyzer unavailable ({reason})");
      return;
    }
  };
  let work = tempfile::tempdir().unwrap();
  let root = canonical_path(&work.path().join("tiny")).unwrap();
  fs::create_dir_all(root.join("src")).unwrap();
  fs::write(root.join("Cargo.toml"), "[package]\nname = \"tiny\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
  let lib = root.join("src/lib.rs");
  let caller = root.join("src/caller.rs");
  let lib_text = "pub mod caller;\n\npub fn answer() -> u32 {\n  42\n}\n";
  fs::write(&lib, lib_text).unwrap();
  fs::write(&caller, "pub fn ask() -> u32 {\n  crate::answer()\n}\n").unwrap();
  let settings = RustAnalyzerSettings::new(vec![root.join("Cargo.toml")], canonical_path(&work.path().join("lsp-target")).unwrap());
  let unit = ServerUnit::new(
    "tiny",
    vec![UnitFolder::new("tiny", &root)],
    UnitSettings::RustAnalyzer(settings),
    ServerCommand::new(binary).cwd(&root),
    Launch::Installed,
  );
  let pool = LspPool::new(PoolOptions::one_shot());

  let lease = pool.get(&unit, Some(RunId(1))).unwrap();

  assert_eq!(lease.state(), ServerState::Ready);
  assert_eq!(lease.server_info().map(|info| info.name.as_str()), Some("rust-analyzer"));
  let symbols: Option<DocumentSymbolResponse> = lease
    .request(
      "textDocument/documentSymbol",
      DocumentSymbolParams {
        text_document: TextDocumentIdentifier {
          uri: uri_from_path(&lib).unwrap(),
        },
      },
    )
    .unwrap();
  let Some(DocumentSymbolResponse::Nested(symbols)) = symbols else {
    panic!("expected hierarchical symbols, got {symbols:?}");
  };
  let names: Vec<&str> = symbols.iter().map(|symbol| symbol.name.as_str()).collect();
  assert_eq!(names, vec!["caller", "answer"]);
  assert_eq!(references(&lease, &lib, lib_text, "answer"), vec![(caller.clone(), 1)]);
  drop(lease);

  fs::write(
    &caller,
    "pub fn ask() -> u32 {\n  crate::answer()\n}\n\npub fn twice() -> u32 {\n  crate::answer() * 2\n}\n",
  )
  .unwrap();
  let hash = ContentHash::of(&fs::read(&caller).unwrap());
  let synced = pool
    .sync(
      &unit,
      Some(RunId(2)),
      SyncDemand::Best,
      |since| {
        assert_eq!(since, Some(RunId(1)));
        ChangeList::Known(vec![FileChange::new(&caller, ChangeKind::Changed(hash))])
      },
      &[],
    )
    .unwrap();

  assert_eq!(synced.report.opened, vec![caller.clone()]);
  assert_eq!(synced.report.drifted, Vec::<PathBuf>::new());
  assert_eq!(references(&synced.lease, &lib, lib_text, "answer"), vec![(caller.clone(), 1), (caller.clone(), 5)]);
  let lockfile = root.join("Cargo.lock");
  let expected_lockfile_changes = match lockfile.exists() {
    true => vec![lockfile],
    false => vec![],
  };
  assert_eq!(synced.lease.lockfile_changes(), expected_lockfile_changes);
}
