use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use atlas_core::Span;
use atlas_lsp::protocol::{
  ClientCapabilities, DidOpenTextDocumentParams, DocumentSymbolParams, DocumentSymbolResponse, InitializeParams, TextDocumentIdentifier, TextDocumentItem, WorkspaceFolder,
};
use atlas_lsp::{Client, ClientOptions, LineBreaks, LineIndex, NoopHandler, PositionEncoding, ProcessExit, ServerCommand, ShutdownOutcome, canonical_path, uri_from_path};
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
