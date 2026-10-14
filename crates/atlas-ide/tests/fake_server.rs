mod common;

use atlas_ide::{DeclarationRequest, IdeError, IdeSettings, Outline, OutlineRequest, ReadSource, References, ReferencesRequest};
use atlas_lsp::ServerKind;
use atlas_query::{Location, OutlineAnswer, OutlineParse, RenderText, SymbolSource, Targeted, TextOut};
use common::*;
use serde_json::json;

const MONEY: &str = "export function toCents(amount: string): number {\n  return Math.round(Number(amount) * 100);\n}\n";
const USE: &str =
  "import { toCents } from './money';\r\n// naïve 日本 \r comment\r\nexport function total(): number {\r\n  const é = '😀'; return toCents('1') + toCents(é);\r\n}\r\n";

fn web() -> Fixture {
  Fixture::new(&[(
    "web",
    &[
      ("package.json", "{\"name\": \"web\"}\n"),
      ("tsconfig.json", "{}\n"),
      ("src/money.ts", MONEY),
      ("src/use.ts", USE),
    ],
  )])
}

#[test]
fn should_map_typescript_references_from_utf16_positions_with_crlf_and_lone_cr_lines() {
  let fixture = web();
  let use_uri = fixture.uri("web", "src/use.ts");
  let money_uri = fixture.uri("web", "src/money.ts");
  let first = utf16_position(USE, "toCents", 1);
  let second = utf16_position(USE, "toCents", 2);
  let import = utf16_position(USE, "toCents", 0);
  let declaration = utf16_position(MONEY, "toCents", 0);
  let references = json!([
    location(&use_uri, second, (second.0, second.1 + 7)),
    location(&money_uri, declaration, (declaration.0, declaration.1 + 7)),
    location(&use_uri, import, (import.0, import.1 + 7)),
    location(&use_uri, first, (first.0, first.1 + 7)),
    location(&use_uri, first, (first.0, first.1 + 7)),
  ]);
  let command = fixture.script(
    "ts",
    &with(
      ts_ready(capabilities(json!({}))),
      [fixture.record("ts"), json!({"canned": {"method": "textDocument/references", "result": references}})],
    ),
  );
  fixture.serve(ServerKind::TypeScript, command);

  let response = fixture.ide.references(&ReferencesRequest::new("toCents")).unwrap();

  assert_eq!((first, second, import, declaration), ((4, 25), (4, 40), (0, 9), (0, 16)));
  assert_eq!(
    text(&response),
    concat!(
      "references toCents function web:src/money.ts:1:17  (typescript-language-server 5.1.3, TypeScript 5.9.3) [compiler]\n",
      "web (3)\n",
      "  <file>  src/use.ts:1:10  import { toCents } from './money';\n",
      "  total  src/use.ts:4:29  const é = '😀'; return toCents('1') + toCents(é);\n",
      "  total  src/use.ts:4:44  const é = '😀'; return toCents('1') + toCents(é);\n",
      "outside the language server's view:\n",
      "  (none)\n",
    )
  );
  let recorded = fixture.recorded("ts");
  let request = recorded.iter().find(|message| message["method"] == "textDocument/references").unwrap();
  assert_eq!(
    request["params"],
    json!({"textDocument": {"uri": money_uri}, "position": {"line": 0, "character": 16}, "context": {"includeDeclaration": false}})
  );
  let References::Exact(answer) = &response.answer else {
    panic!("expected an exact answer")
  };
  let Targeted::Found(answer) = answer.as_ref() else { panic!("expected a found target") };
  assert_eq!(answer.server.certainty.source, "lsp:typescript-language-server 5.1.3");
  assert_eq!(answer.server.certainty.class, "compiler_verified");
}

fn lsp_position(text: &str, line: u32, column: u32) -> (u32, u32) {
  let start: usize = text.split_inclusive('\n').take(line as usize - 1).map(str::len).sum();
  (line - 1, text[start..start + column as usize - 1].encode_utf16().count() as u32)
}

#[test]
fn should_list_graph_sites_the_server_did_not_return_outside_its_view() {
  let fixture = Fixture::copied("acme", &["acme-api", "acme-sdk", "acme-web", "acme-billing"]);
  let query = fixture.ide.query();
  query.fresh().unwrap();
  let Targeted::Found(target) = query.resolve_target("toCents").unwrap() else {
    panic!("toCents not found")
  };
  let graph = query.graph_references(&target).unwrap();
  let mut returned = Vec::new();
  for row in &graph.references {
    let Some(location) = &row.location else { continue };
    if location.repo != "acme-sdk" {
      continue;
    }
    let file = fixture.file("acme-sdk", &location.path);
    let source = std::fs::read_to_string(&file).unwrap();
    let start = lsp_position(&source, location.line.unwrap(), location.column.unwrap());
    returned.push(location_json(&atlas_lsp::uri_from_path(&file).unwrap(), start));
  }
  let command = fixture.script(
    "ts",
    &with(
      ts_ready(capabilities(json!({}))),
      [json!({"canned": {"method": "textDocument/references", "result": returned}})],
    ),
  );
  fixture.serve(ServerKind::TypeScript, command);

  let response = fixture.ide.references(&ReferencesRequest::new("toCents")).unwrap();

  let rendered = text(&response);
  let outside: Vec<&str> = rendered.lines().skip_while(|line| !line.starts_with("outside")).collect();
  assert_eq!(
    outside,
    vec![
      "outside the language server's view:",
      "  other repo [heuristic 0.85]  acme-web:src/components/OrderForm.tsx:1:10  src/components/OrderForm.tsx  import { toCents } from '@acme/sdk';",
      "  other repo [heuristic 0.85]  acme-web:src/components/OrderForm.tsx:15:44  OrderForm  submit({ userId: me.id, subtotalCents: toCents(amount) });",
    ],
    "{rendered}"
  );
  assert!(rendered.starts_with("references toCents function acme-sdk:src/money.ts:1:17"), "{rendered}");
  assert!(rendered.contains("acme-sdk (6)\n"), "{rendered}");
  assert_eq!(returned.len(), 6);
}

fn location_json(uri: &str, start: (u32, u32)) -> serde_json::Value {
  location(uri, start, (start.0, start.1 + 7))
}

#[test]
fn should_answer_an_ambiguous_name_with_its_candidates_without_starting_a_server() {
  let fixture = Fixture::new(&[(
    "web",
    &[
      ("package.json", "{\"name\": \"web\"}\n"),
      ("src/a.ts", "export function helper() {}\n"),
      ("src/b.ts", "export function helper() {}\n"),
    ],
  )]);

  let response = fixture.ide.references(&ReferencesRequest::new("helper")).unwrap();

  let rendered = text(&response);
  assert!(rendered.starts_with("ambiguous target \"helper\": 2 candidates"), "{rendered}");
  assert!(
    rendered.contains("helper function web:src/a.ts:1") && rendered.contains("helper function web:src/b.ts:1"),
    "{rendered}"
  );
}

#[test]
fn should_refuse_endpoints_files_and_paths_outside_the_project() {
  let fixture = web();
  let error = |target: &str| fixture.ide.references(&ReferencesRequest::new(target)).unwrap_err();

  assert_eq!(
    error("GET /users/:id").to_string(),
    "invalid query: \"GET /users/:id\" is an endpoint; endpoints have no LSP symbol; use atlas_api"
  );
  assert!(matches!(error("/elsewhere/a.ts:1:1"), IdeError::OutsideProject { ref path, .. } if path == "/elsewhere/a.ts"));
  assert!(matches!(error("web:../other/a.ts:1:1"), IdeError::OutsideProject { .. }));
  assert_eq!(
    error("src/money.ts").to_string(),
    "\"src/money.ts\" is a file, not a code symbol; give a symbol name, a key or [repo:]path:line:col"
  );
  assert_eq!(error("nope:src/money.ts:1:1").to_string(), "unknown repo \"nope\"; registered repos: web");
  assert!(matches!(error("src/absent.ts:1:1"), IdeError::FileNotFound { .. }));
}

#[test]
fn should_fail_without_a_server_and_point_read_modes_to_the_graph() {
  let fixture = web();

  let references = fixture.ide.references(&ReferencesRequest::new("toCents")).unwrap_err().to_string();
  let declaration = fixture
    .ide
    .declaration(&DeclarationRequest::Position {
      target: "web:src/use.ts:4:29".to_owned(),
    })
    .unwrap_err()
    .to_string();
  let graph = fixture
    .ide
    .references(&ReferencesRequest {
      source: ReadSource::Graph,
      ..ReferencesRequest::new("toCents")
    })
    .unwrap();

  assert_eq!(
    references,
    "typescript-language-server is unavailable: typescript-language-server not found. Install it (npm i -g typescript-language-server typescript@5); retry with source='graph' for tagged graph results"
  );
  assert_eq!(
    declaration,
    "typescript-language-server is unavailable: typescript-language-server not found. Install it (npm i -g typescript-language-server typescript@5)"
  );
  assert!(matches!(graph.answer, References::Graph(_)));
  assert!(text(&graph).contains("graph references are not exhaustive"), "{}", text(&graph));
}

#[test]
fn should_refuse_exact_modes_when_language_servers_are_disabled() {
  let fixture = web();
  fixture.ide.reload_settings(IdeSettings { enabled: false, ..settings() });

  let outline = fixture.ide.outline(&lsp_outline("src/money.ts")).unwrap_err().to_string();
  let graph = fixture
    .ide
    .outline(&OutlineRequest {
      source: ReadSource::Graph,
      ..OutlineRequest::new("src/money.ts")
    })
    .unwrap();

  assert_eq!(
    outline,
    "language servers are disabled for project demo (lsp.enabled = false); enable them with `catl project config demo --lsp on`; retry with source='graph' for tagged graph results"
  );
  assert!(text(&graph).starts_with("outline web:src/money.ts source=graph"), "{}", text(&graph));
}

#[test]
fn should_find_the_declaration_at_a_regex_capture_through_a_location_link() {
  let fixture = web();
  let money_uri = fixture.uri("web", "src/money.ts");
  let link = json!([{
    "targetUri": money_uri,
    "targetRange": range((0, 0), (2, 1)),
    "targetSelectionRange": range((0, 16), (0, 23))
  }]);
  let command = fixture.script(
    "ts",
    &with(
      ts_ready(capabilities(json!({}))),
      [fixture.record("ts"), json!({"canned": {"method": "textDocument/definition", "result": link}})],
    ),
  );
  fixture.serve(ServerKind::TypeScript, command);

  let response = fixture
    .ide
    .declaration(&DeclarationRequest::Regex {
      path: "web:src/use.ts".to_owned(),
      regex: r"\+ (toCents)\(".to_owned(),
    })
    .unwrap();

  assert_eq!(
    text(&response),
    concat!(
      "declaration at web:src/use.ts:4:44  (typescript-language-server 5.1.3, TypeScript 5.9.3) [compiler]\n",
      "  definition  toCents  web:src/money.ts:1:17  export function toCents(amount: string): number {\n",
    )
  );
  let recorded = fixture.recorded("ts");
  let request = recorded.iter().find(|message| message["method"] == "textDocument/definition").unwrap();
  assert_eq!(request["params"]["position"], json!({"line": 4, "character": 40}));
  assert_eq!(methods(&recorded).iter().filter(|method| *method == "textDocument/declaration").count(), 0);
}

#[test]
fn should_list_implementations_and_outline_a_file_from_the_server() {
  let fixture = web();
  let use_uri = fixture.uri("web", "src/use.ts");
  let symbols = json!([{
    "name": "total", "kind": 12, "range": range((3, 0), (5, 1)), "selectionRange": range((3, 16), (3, 21)),
    "children": [{"name": "é", "kind": 14, "detail": "const", "range": range((4, 8), (4, 16)), "selectionRange": range((4, 8), (4, 9))}]
  }]);
  let command = fixture.script(
    "ts",
    &with(
      ts_ready(capabilities(json!({}))),
      [
        json!({"canned": {"method": "textDocument/implementation", "result": [location(&use_uri, (3, 16), (3, 21))]}}),
        json!({"canned": {"method": "textDocument/documentSymbol", "result": symbols}}),
      ],
    ),
  );
  fixture.serve(ServerKind::TypeScript, command);

  let implementations = fixture.ide.implementations("toCents").unwrap();
  let outline = fixture.ide.outline(&lsp_outline("web:src/use.ts")).unwrap();
  let shallow = fixture
    .ide
    .outline(&OutlineRequest {
      depth: 0,
      ..lsp_outline("src/use.ts")
    })
    .unwrap();

  assert_eq!(
    text(&implementations),
    concat!(
      "implementations of toCents function web:src/money.ts:1:17  (typescript-language-server 5.1.3, TypeScript 5.9.3) [compiler]\n",
      "  web  total  src/use.ts:3:17  export function total(): number {\n",
    )
  );
  assert_eq!(
    text(&outline),
    concat!(
      "outline web:src/use.ts  (typescript-language-server 5.1.3, TypeScript 5.9.3) [compiler]\n",
      "  total function 3:17\n",
      "    é constant 4:9  const\n",
    )
  );
  assert_eq!(
    text(&shallow),
    "outline web:src/use.ts  (typescript-language-server 5.1.3, TypeScript 5.9.3) [compiler]\n  total function 3:17\n"
  );
}

#[test]
fn should_query_a_typescript_overload_set_at_its_first_signature() {
  let source =
    "export function pad(value: string): string;\nexport function pad(value: number): string;\nexport function pad(value: unknown): string {\n  return String(value);\n}\n";
  let fixture = Fixture::new(&[("web", &[("package.json", "{\"name\": \"web\"}\n"), ("src/pad.ts", source)])]);
  let command = fixture.script(
    "ts",
    &with(
      ts_ready(capabilities(json!({}))),
      [fixture.record("ts"), json!({"canned": {"method": "textDocument/references", "result": null}})],
    ),
  );
  fixture.serve(ServerKind::TypeScript, command);

  let response = fixture.ide.references(&ReferencesRequest::new("pad")).unwrap();

  assert!(text(&response).starts_with("references pad function web:src/pad.ts:1:17  ("), "{}", text(&response));
  assert!(text(&response).contains("  (no references)\n"), "{}", text(&response));
  let recorded = fixture.recorded("ts");
  let request = recorded.iter().find(|message| message["method"] == "textDocument/references").unwrap();
  assert_eq!(request["params"]["position"], json!({"line": 0, "character": 16}));
}

#[test]
fn should_keep_the_site_of_a_position_target_and_say_graph_extras_need_a_symbol() {
  let fixture = web();
  let use_uri = fixture.uri("web", "src/use.ts");
  let command = fixture.script(
    "ts",
    &with(
      ts_ready(capabilities(json!({}))),
      [
        fixture.record("ts"),
        json!({"canned": {"method": "textDocument/references", "result": [location(&use_uri, (4, 25), (4, 32)), location(&use_uri, (4, 40), (4, 47))]}}),
      ],
    ),
  );
  fixture.serve(ServerKind::TypeScript, command);

  let response = fixture.ide.references(&ReferencesRequest::new("web:src/use.ts:4:29")).unwrap();

  assert_eq!(
    text(&response),
    concat!(
      "references at web:src/use.ts:4:29  (typescript-language-server 5.1.3, TypeScript 5.9.3) [compiler]\n",
      "web (2)\n",
      "  total  src/use.ts:4:29  const é = '😀'; return toCents('1') + toCents(é);\n",
      "  total  src/use.ts:4:44  const é = '😀'; return toCents('1') + toCents(é);\n",
      "outside the language server's view:\n",
      "  (none)\n",
      "caveats:\n",
      "  sites outside the language server's view are only listed for a symbol target (the position is not on an indexed symbol's name)\n",
    )
  );
  let recorded = fixture.recorded("ts");
  let request = recorded.iter().find(|message| message["method"] == "textDocument/references").unwrap();
  assert_eq!(request["params"]["position"], json!({"line": 4, "character": 25}));
}

#[test]
fn should_list_resolved_same_repo_sites_outside_the_server_project() {
  let fixture = Fixture::new(&[(
    "web",
    &[
      ("package.json", "{\"name\": \"web\"}\n"),
      ("tsconfig.json", "{\"include\": [\"src\"]}\n"),
      ("src/money.ts", MONEY),
      ("src/use.ts", USE),
      (
        "scripts/seed.ts",
        "import { toCents } from '../src/money';\n\nexport function seed(): number {\n  return toCents('5');\n}\n",
      ),
    ],
  )]);
  let use_uri = fixture.uri("web", "src/use.ts");
  let returned: Vec<serde_json::Value> = [0, 1, 2]
    .iter()
    .map(|nth| {
      let start = utf16_position(USE, "toCents", *nth);
      location(&use_uri, start, (start.0, start.1 + 7))
    })
    .collect();
  let command = fixture.script(
    "ts",
    &with(
      ts_ready(capabilities(json!({}))),
      [json!({"canned": {"method": "textDocument/references", "result": returned}})],
    ),
  );
  fixture.serve(ServerKind::TypeScript, command);

  let response = fixture.ide.references(&ReferencesRequest::new("toCents")).unwrap();

  let rendered = text(&response);
  let outside: Vec<&str> = rendered.lines().skip_while(|line| !line.starts_with("outside")).collect();
  assert_eq!(
    outside,
    vec![
      "outside the language server's view:",
      "  not in the server's project [resolved]  web:scripts/seed.ts:1:10  scripts/seed.ts  import { toCents } from '../src/money';",
      "  not in the server's project [resolved]  web:scripts/seed.ts:4:10  seed  return toCents('5');",
    ],
    "{rendered}"
  );
}

#[test]
fn should_list_http_consumers_at_their_call_sites() {
  let fixture = Fixture::copied("acme", &["acme-api", "acme-sdk", "acme-web", "acme-billing"]);
  let command = fixture.script(
    "ts",
    &with(ts_ready(capabilities(json!({}))), [json!({"canned": {"method": "textDocument/references", "result": []}})]),
  );
  fixture.serve(ServerKind::TypeScript, command);

  let response = fixture.ide.references(&ReferencesRequest::new("UsersController.findOne")).unwrap();

  let rendered = text(&response);
  let http: Vec<&str> = rendered.lines().filter(|line| line.starts_with("  http ")).collect();
  assert_eq!(
    http,
    vec![
      "  http GET /users/{id} [heuristic 0.77]  acme-sdk:src/client.ts:20  AcmeClient.getUser  const response = await fetch(`${this.baseUrl}/users/${id}`, { headers: this.headers() });",
      "  http GET /users/{id} [heuristic 0.47]  acme-web:src/hooks/useMe.ts:12  useMe  fetch(`${API_URL}/users/me`, { headers: { Authorization: `Bearer ${token}` } })",
    ],
    "{rendered}"
  );
}

#[test]
fn should_list_jest_mocks_of_orders_service_find_one_outside_the_server_view() {
  let fixture = Fixture::copied("acme", &["acme-api", "acme-sdk", "acme-web", "acme-billing"]);
  let controller = fixture.uri("acme-api", "src/orders/orders.controller.ts");
  let command = fixture.script(
    "ts",
    &with(
      ts_ready(capabilities(json!({}))),
      [json!({"canned": {"method": "textDocument/references", "result": [location(&controller, (15, 30), (15, 37))]}})],
    ),
  );
  fixture.serve(ServerKind::TypeScript, command);

  let response = fixture.ide.references(&ReferencesRequest::new("OrdersService.findOne")).unwrap();

  let rendered = text(&response);
  assert!(
    rendered.starts_with("references OrdersService.findOne method acme-api:src/orders/orders.service.ts:23:9  ("),
    "{rendered}"
  );
  assert_eq!(
    rendered.lines().skip(1).collect::<Vec<_>>(),
    vec![
      "acme-api (1)",
      "  OrdersController.findOne  src/orders/orders.controller.ts:16:31  return this.ordersService.findOne(id);",
      "outside the language server's view:",
      "  mock (replaced) [heuristic 0.60]  acme-api:src/orders/orders.controller.spec.ts:5:27  OrdersController  const ordersService = { findOne: jest.fn(), create: jest.fn() };",
      "  mock (replaced) [heuristic 0.60]  acme-api:src/orders/orders.controller.spec.ts:11:19  OrdersController.should delegate findOne to the service  ordersService.findOne.mockResolvedValue({ id: 'o1', display: '$12.00' });",
      "  mock (replaced) [heuristic 0.60]  acme-api:src/orders/orders.controller.spec.ts:13:26  OrdersController.should delegate findOne to the service  expect(ordersService.findOne).toHaveBeenCalledWith('o1');",
    ],
    "{rendered}"
  );
}

const OUTLINE_HEADER: &str = "outline web:src/money.ts source=graph (from the index, syntactic; for the exact language-server outline use source=lsp)\n";

#[test]
fn should_outline_from_the_graph_by_default_without_starting_a_server() {
  let fixture = web();

  let request = OutlineRequest::new("src/money.ts");
  let outline = fixture.ide.outline(&request).unwrap();

  assert_eq!(request.source, ReadSource::Graph);
  assert_eq!(text(&outline), format!("{OUTLINE_HEADER}  toCents function :1  function toCents(amount: string): number\n"));
}

#[test]
fn should_outline_from_the_server_only_when_source_lsp_is_explicit() {
  let fixture = web();
  fixture.ide.reload_settings(IdeSettings { enabled: false, ..settings() });

  let graph = fixture.ide.outline(&OutlineRequest::new("src/money.ts")).unwrap();
  let lsp = fixture.ide.outline(&lsp_outline("src/money.ts")).unwrap_err().to_string();

  assert!(text(&graph).contains(OUTLINE_HEADER), "{}", text(&graph));
  assert!(lsp.starts_with("language servers are disabled for project demo"), "{lsp}");
}

#[test]
fn should_say_the_graph_has_no_facts_and_suggest_the_language_server_instead_of_falling_back() {
  let fixture = web();

  let outline = fixture.ide.outline(&OutlineRequest::new("src/missing.ts")).unwrap();

  assert_eq!(
    text(&outline).lines().collect::<Vec<_>>(),
    vec![
      "no graph facts for \"src/missing.ts\" (no indexed file at src/missing.ts); the file may be excluded or not indexed; the language server can still outline it: use source=lsp"
    ]
  );
}

#[test]
fn should_state_partial_and_failed_parses_in_the_outline_header() {
  let answer = |parse| {
    Outline::Graph(Targeted::Found(OutlineAnswer {
      file: Location {
        repo: "web".to_owned(),
        path: "src/money.ts".to_owned(),
        line: None,
        column: None,
      },
      source: SymbolSource::Graph,
      parse,
      symbols: Vec::new(),
      caveats: Vec::new(),
    }))
  };
  let lines = |outline: Outline| {
    let mut out = TextOut::default();
    outline.render_text(&mut out);
    out.finish()
  };

  assert_eq!(
    lines(answer(OutlineParse::Partial { error_count: 2 })),
    format!("{OUTLINE_HEADER}PARTIAL: the file parsed with 2 errors; symbols near the errors may be missing\n  (no symbols)\n")
  );
  assert_eq!(
    lines(answer(OutlineParse::Failed {
      reason: "parse budget exceeded".to_owned()
    })),
    format!("{OUTLINE_HEADER}NO FACTS: the file failed to parse (parse budget exceeded), so the graph has no symbols for it; use source=lsp\n")
  );
}

#[test]
fn should_mark_the_outline_of_a_partially_parsed_file_in_its_header() {
  let fixture = Fixture::new(&[(
    "web",
    &[
      ("package.json", "{\"name\": \"web\"}\n"),
      ("tsconfig.json", "{}\n"),
      ("src/broken.ts", "export function ok(): number {\n  return 1;\n}\nfunction broken( { return ;\n"),
    ],
  )]);

  let outline = fixture.ide.outline(&OutlineRequest::new("src/broken.ts")).unwrap();

  let rendered = text(&outline);
  assert!(rendered.starts_with("outline web:src/broken.ts source=graph (from the index, syntactic;"), "{rendered}");
  assert!(rendered.lines().nth(1).is_some_and(|line| line.starts_with("PARTIAL: the file parsed with ")), "{rendered}");
}
