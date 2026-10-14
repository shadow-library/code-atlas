mod common;

use atlas_core::{EntityKind, RelPath, RepoName};
use atlas_query::{FindRequest, GraphReferences, HierarchyRequest, NameMatch, NameSpan, OutlineRequest, QueryError, ReferenceSide, ReferencesRequest, Targeted, render_json};

const USERS_FIND_ONE: &str = "ts:acme-api:acme-api:src/users/users.service::UsersService::findOne#method";

fn found<T: std::fmt::Debug>(answer: Targeted<T>) -> T {
  match answer {
    Targeted::Found(found) => found,
    other => panic!("{other:?}"),
  }
}

fn rows(references: &GraphReferences) -> Vec<String> {
  references
    .references
    .iter()
    .map(|reference| {
      let location = reference.location.as_ref().map_or_else(|| "-".to_owned(), ToString::to_string);
      let enclosing = reference.enclosing.as_ref().map_or("(file)", |enclosing| enclosing.short.as_str());
      format!("{location} {enclosing} {} {}", reference.phrase, reference.certainty.tag)
    })
    .collect()
}

fn substring(pattern: &str) -> FindRequest {
  let mut request = FindRequest::new(pattern);
  request.name_match = NameMatch::Substring;
  request
}

#[test]
fn should_find_users_service_find_one_by_name_path() {
  let fixture = common::acme();

  let response = fixture.query.find(&FindRequest::new("UsersService/findOne")).unwrap();

  assert_eq!(
    fixture.text(&response),
    "fresh run=1 (+58 files, <t>)\n\
     find \"UsersService/findOne\": 2 matches\n  \
       UsersService/findOne method acme-api:src/users/users.service.ts:18  async findOne(id: string): Promise<User>\n  \
       UsersService/findOne test_suite acme-api:src/users/users.service.spec.ts:12  describe('findOne')\n"
  );
  assert_eq!(response.answer.matches.items[0].entity.key, USERS_FIND_ONE);
}

#[test]
fn should_accept_dotted_and_absolute_name_paths() {
  let fixture = common::acme();

  let dotted = fixture.query.find(&FindRequest::new("UsersService.findOne")).unwrap();
  let absolute = fixture.query.find(&FindRequest::new("/UsersService/findOne")).unwrap();
  let not_top_level = fixture.query.find(&FindRequest::new("/findOne")).unwrap();

  assert_eq!(dotted.answer.matches.total, 2);
  assert_eq!(absolute.answer.matches.total, 2);
  assert_eq!(not_top_level.answer.matches.total, 0);
}

#[test]
fn should_find_symbols_by_substring_within_a_repo_directory() {
  let fixture = common::acme();
  let mut request = substring("Order");
  request.path = Some("acme-sdk:src".to_owned());

  let response = fixture.query.find(&request).unwrap();

  assert_eq!(
    fixture.text(&response),
    "fresh run=1 (+58 files, <t>)\n\
     find \"Order\": 4 matches\n  \
       AcmeClient/createOrder method acme-sdk:src/client.ts:29  async createOrder(input: CreateOrderInput): Promise<Order>\n  \
       AcmeClient/getOrder method acme-sdk:src/client.ts:34  async getOrder(id: string): Promise<Order>\n  \
       Order interface acme-sdk:src/types.ts:14  interface Order\n  \
       CreateOrderInput interface acme-sdk:src/types.ts:20  interface CreateOrderInput\n"
  );
}

#[test]
fn should_filter_found_symbols_by_kind() {
  let fixture = common::acme();
  let mut interfaces = substring("Order");
  interfaces.kinds = vec![EntityKind::Interface];
  let mut no_methods = substring("Order");
  no_methods.path = Some("acme-sdk:".to_owned());
  no_methods.exclude_kinds = vec![EntityKind::Method];

  let interfaces = fixture.query.find(&interfaces).unwrap();
  let no_methods = fixture.query.find(&no_methods).unwrap();

  let keys = |answer: &atlas_query::FindAnswer| answer.matches.items.iter().map(|found| found.entity.key.clone()).collect::<Vec<_>>();
  assert_eq!(
    keys(&interfaces.answer),
    vec![
      "ts:acme-api:acme-api:src/orders/orders.service::CreateOrderInput#interface",
      "ts:acme-sdk:@acme/sdk:src/types::Order#interface",
      "ts:acme-sdk:@acme/sdk:src/types::CreateOrderInput#interface",
    ]
  );
  assert_eq!(
    keys(&no_methods.answer),
    vec![
      "ts:acme-sdk:@acme/sdk:src/types::Order#interface",
      "ts:acme-sdk:@acme/sdk:src/types::CreateOrderInput#interface"
    ]
  );
}

#[test]
fn should_list_the_members_of_a_class_at_depth_one() {
  let fixture = common::acme();
  let mut request = FindRequest::new("AcmeClient");
  request.kinds = vec![EntityKind::Class];
  request.depth = 1;

  let response = fixture.query.find(&request).unwrap();

  assert_eq!(
    fixture.text(&response),
    "fresh run=1 (+58 files, <t>)\n\
     find \"AcmeClient\": 1 match\n  \
       AcmeClient class acme-sdk:src/client.ts:3  class AcmeClient\n    \
         baseUrl field :4  private readonly baseUrl: string\n    \
         auth field :5  private readonly auth: AcmeAuth\n    \
         constructor method :7  constructor(options: AcmeClientOptions)\n    \
         headers method :12  private headers(): Record<string, string>\n    \
         getUser method :19  async getUser(id: string): Promise<User>\n    \
         updateUser method :24  async updateUser(id: string, patch: Partial<Pick<User, 'email' | 'displayName'>>): Promise<User>\n    \
         createOrder method :29  async createOrder(input: CreateOrderInput): Promise<Order>\n    \
         getOrder method :34  async getOrder(id: string): Promise<Order>\n    \
         parse method :39  private async parse<T>(response: Response): Promise<T>\n"
  );
}

#[test]
fn should_include_the_body_when_requested() {
  let fixture = common::acme();
  let mut request = FindRequest::new("toCents");
  request.kinds = vec![EntityKind::Function];
  request.body = true;

  let text = fixture.text(&fixture.query.find(&request).unwrap());

  assert!(text.ends_with("    6|   return negative ? -cents : cents;\n    7| }\n"), "{text}");
  assert!(
    text.contains("  toCents function acme-sdk:src/money.ts:1  function toCents(amount: string): number\n    1| export function toCents"),
    "{text}"
  );
}

#[test]
fn should_reject_out_of_range_find_options() {
  let fixture = common::acme();
  let mut too_many = FindRequest::new("x");
  too_many.max_matches = 201;
  let mut too_deep = FindRequest::new("x");
  too_deep.depth = 4;
  let mut not_a_symbol = FindRequest::new("x");
  not_a_symbol.kinds = vec![EntityKind::ApiEndpoint];

  for request in [too_many, too_deep, not_a_symbol] {
    assert!(matches!(fixture.query.find(&request), Err(QueryError::Invalid(_))), "{request:?}");
  }
}

#[test]
fn should_note_a_path_that_matches_no_indexed_file() {
  let fixture = common::acme();
  let mut request = FindRequest::new("toCents");
  request.path = Some("acme-sdk:src/nowhere".to_owned());

  let text = fixture.text(&fixture.query.find(&request).unwrap());

  assert!(
    text.ends_with("find \"toCents\": no matches\ncaveats:\n  no indexed file matches the path \"acme-sdk:src/nowhere\"\n"),
    "{text}"
  );
}

#[test]
fn should_outline_the_sdk_client_from_the_graph() {
  let fixture = common::acme();

  let response = fixture.query.outline(&OutlineRequest::new("acme-sdk:src/client.ts")).unwrap();

  assert_eq!(
    fixture.text(&response),
    "fresh run=1 (+58 files, <t>)\n\
     outline acme-sdk:src/client.ts source=graph (from the index, syntactic; for the exact language-server outline use source=lsp)\n  \
       AcmeClient class :3  class AcmeClient\n    \
         baseUrl field :4  private readonly baseUrl: string\n    \
         auth field :5  private readonly auth: AcmeAuth\n    \
         constructor method :7  constructor(options: AcmeClientOptions)\n    \
         headers method :12  private headers(): Record<string, string>\n    \
         getUser method :19  async getUser(id: string): Promise<User>\n    \
         updateUser method :24  async updateUser(id: string, patch: Partial<Pick<User, 'email' | 'displayName'>>): Promise<User>\n    \
         createOrder method :29  async createOrder(input: CreateOrderInput): Promise<Order>\n    \
         getOrder method :34  async getOrder(id: string): Promise<Order>\n    \
         parse method :39  private async parse<T>(response: Response): Promise<T>\n"
  );
  let json: serde_json::Value = serde_json::from_str(&render_json(&response).unwrap()).unwrap();
  assert_eq!(json["answer"]["source"], "graph");
  assert_eq!(json["answer"]["symbols"][0]["children"].as_array().map(Vec::len), Some(9));
}

#[test]
fn should_outline_only_top_level_symbols_at_depth_zero_and_report_unknown_files() {
  let fixture = common::acme();

  let shallow = found(
    fixture
      .query
      .outline(&OutlineRequest {
        file: "src/client.ts".to_owned(),
        depth: 0,
      })
      .unwrap()
      .answer,
  );
  let missing = fixture.query.outline(&OutlineRequest::new("acme-sdk:src/missing.ts")).unwrap();

  assert_eq!(shallow.symbols.len(), 1);
  assert!(shallow.symbols[0].children.is_empty());
  assert_eq!(
    missing.answer,
    Targeted::NotFound {
      spec: "acme-sdk:src/missing.ts".to_owned(),
      hint: "no indexed file at acme-sdk:src/missing.ts".to_owned()
    }
  );
}

#[test]
fn should_list_every_users_service_find_one_call_with_its_name_column() {
  let fixture = common::acme();

  let response = fixture.query.symbol_references(&ReferencesRequest::new("UsersService/findOne")).unwrap();

  let answer = found(response.answer.clone());
  assert_eq!(
    rows(&answer),
    vec![
      "acme-api:src/users/users.controller.ts:16:30 UsersController.findOne calls [resolved]",
      "acme-api:src/users/users.service.spec.ts:15:28 UsersService.findOne.should return the user when it exists calls [resolved]",
      "acme-api:src/users/users.service.spec.ts:20:28 UsersService.findOne.should throw NotFoundException when the user is missing calls [resolved]",
      "acme-api:src/users/users.service.ts:29:29 UsersService.update calls [resolved]",
    ]
  );
  assert!(answer.unresolved.is_empty());
  assert!(answer.unverified_files.is_empty());
  assert_eq!(fixture.text(&response).lines().nth(2), Some("acme-api (4)"),);
  assert_eq!(
    fixture.text(&response).lines().nth(3),
    Some("  UsersController.findOne  src/users/users.controller.ts:16:30  calls [resolved]  return this.usersService.findOne(id);")
  );
}

#[test]
fn should_include_the_cross_repo_sdk_import_and_call_of_to_cents() {
  let fixture = common::acme();

  let answer = found(fixture.query.symbol_references(&ReferencesRequest::new("toCents")).unwrap().answer);

  assert_eq!(
    rows(&answer),
    vec![
      "acme-sdk:src/index.ts:3:10 (file) re-exports [resolved]",
      "acme-sdk:src/money.test.ts:3:10 (file) imports [resolved]",
      "acme-sdk:src/money.test.ts:7:12 toCents.should convert whole and fractional amounts calls [resolved]",
      "acme-sdk:src/money.test.ts:8:12 toCents.should convert whole and fractional amounts calls [resolved]",
      "acme-sdk:src/money.test.ts:9:12 toCents.should convert whole and fractional amounts calls [resolved]",
      "acme-sdk:src/money.test.ts:13:12 toCents.should keep the sign of negative amounts calls [resolved]",
      "acme-web:src/components/OrderForm.tsx:1:10 (file) imports [resolved]",
      "acme-web:src/components/OrderForm.tsx:15:44 OrderForm calls [resolved]",
    ]
  );
  let web_call = &answer.references[7];
  assert_eq!(web_call.side, ReferenceSide::Inbound);
  assert_eq!(web_call.edge_kind, "calls");
  assert!(web_call.certainty.authoritative);
  assert_eq!(web_call.snippet.as_deref(), Some("submit({ userId: me.id, subtotalCents: toCents(amount) });"));
}

#[test]
fn should_list_format_money_imports_and_calls() {
  let fixture = common::acme();

  let response = fixture.query.symbol_references(&ReferencesRequest::new("formatMoney")).unwrap();

  assert_eq!(
    fixture.text(&response),
    "fresh run=1 (+58 files, <t>)\n\
     references formatMoney function acme-api:src/common/money.ts:1 source=graph\n\
     acme-api (5)\n  \
       (file)  src/orders/orders.service.ts:7:10  imports [resolved]  import { formatMoney } from '../common/money';\n  \
       OrdersService.findOne  src/orders/orders.service.ts:26:33  calls [resolved]  return { ...order, display: formatMoney(order.totalCents) };\n  \
       (file)  src/reports/reports.service.ts:6:10  imports [resolved]  import { formatMoney } from '../common/money';\n  \
       ReportsService.exportCsv  src/reports/reports.service.ts:15:71  calls [resolved]  const rows = orders.map((order) => `${order.id},${order.userId},${formatMoney(order.totalCents)}`);\n  \
       ReportsService.exportCsv  src/reports/reports.service.ts:17:51  calls [resolved]  return ['id,user,total', ...rows, `average,,${formatMoney(average)}`].join('\\n');\n\
     caveats:\n  \
       graph references are not exhaustive: calls through dynamic dispatch, reflection or unindexed code are missing\n"
  );
}

#[test]
fn should_fall_back_to_the_line_when_a_referencing_file_changed_after_indexing() {
  let fixture = common::acme();
  fixture.query.fresh().unwrap();
  let target = found(fixture.query.resolve_target("UsersService/findOne").unwrap());
  let controller = fixture.repo_root("acme-api").join("src/users/users.controller.ts");
  let mut content = std::fs::read_to_string(&controller).unwrap();
  content.push('\n');
  std::fs::write(&controller, content).unwrap();

  let answer = fixture.query.graph_references(&target).unwrap();

  assert_eq!(rows(&answer)[0], "acme-api:src/users/users.controller.ts:16 UsersController.findOne calls [resolved]");
  assert_eq!(answer.references[0].snippet, None);
  assert_eq!(
    answer.unverified_files.iter().map(ToString::to_string).collect::<Vec<_>>(),
    vec!["acme-api:src/users/users.controller.ts"]
  );
  assert_eq!(
    rows(&answer)[1],
    "acme-api:src/users/users.service.spec.ts:15:28 UsersService.findOne.should return the user when it exists calls [resolved]"
  );
}

#[test]
fn should_report_columns_from_the_edited_content_after_the_barrier() {
  let fixture = common::acme();
  fixture.query.fresh().unwrap();
  let controller = fixture.repo_root("acme-api").join("src/users/users.controller.ts");
  let content = std::fs::read_to_string(&controller).unwrap();
  std::fs::write(&controller, format!("\n{content}")).unwrap();

  let answer = found(fixture.query.symbol_references(&ReferencesRequest::new("UsersService/findOne")).unwrap().answer);

  assert_eq!(rows(&answer)[0], "acme-api:src/users/users.controller.ts:17:30 UsersController.findOne calls [resolved]");
  assert!(answer.unverified_files.is_empty());
}

#[test]
fn should_show_the_invoice_store_trait_hierarchy() {
  let fixture = common::acme();

  let response = fixture.query.hierarchy(&HierarchyRequest::new("InvoiceStore")).unwrap();
  let method = fixture.query.hierarchy(&HierarchyRequest::new("InvoiceStore/find")).unwrap();

  assert_eq!(
    fixture.text(&response),
    "fresh run=1 (+58 files, <t>)\n\
     hierarchy InvoiceStore trait acme-billing:src/store.rs:15\n\
     supertypes:\n  \
       (none)\n\
     subtypes:\n  \
       implemented by PgInvoiceStore struct acme-billing:src/store.rs:20 [resolved]\n  \
       implemented by InMemoryInvoiceStore struct acme-billing:src/store.rs:47 [resolved]\n"
  );
  assert_eq!(
    fixture.text(&method),
    "fresh run=1 (+0 files, <t>)\n\
     hierarchy InvoiceStore::find method acme-billing:src/store.rs:17\n\
     supertypes:\n  \
       (none)\n\
     subtypes:\n  \
       implemented by PgInvoiceStore::<InvoiceStore>::find method acme-billing:src/store.rs:42 [resolved]\n  \
       implemented by InMemoryInvoiceStore::<InvoiceStore>::find method acme-billing:src/store.rs:59 [resolved]\n"
  );
}

#[test]
fn should_walk_up_from_an_implementation_to_its_trait() {
  let fixture = common::acme();

  let answer = found(fixture.query.hierarchy(&HierarchyRequest::new("PgInvoiceStore")).unwrap().answer);

  assert_eq!(answer.supertypes.len(), 1);
  assert_eq!(answer.supertypes[0].entity.key, "rust:acme-billing:acme-billing:store::InvoiceStore#trait");
  assert_eq!(answer.supertypes[0].phrase, "implements");
  assert!(answer.supertypes[0].children.is_empty());
  assert!(answer.subtypes.is_empty());
  assert!(matches!(
    fixture.query.hierarchy(&HierarchyRequest {
      target: "InvoiceStore".into(),
      depth: 0
    }),
    Err(QueryError::Invalid(_))
  ));
}

#[test]
fn should_resolve_targets_by_key_position_line_file_and_name_path() {
  let fixture = common::acme();
  fixture.query.fresh().unwrap();
  let key = |text: &str| match fixture.query.resolve_target(text).unwrap() {
    Targeted::Found(entity) => entity.key.to_string(),
    other => format!("{other:?}"),
  };

  assert_eq!(key(USERS_FIND_ONE), USERS_FIND_ONE);
  assert_eq!(key("acme-api:src/users/users.service.ts:19:5"), USERS_FIND_ONE);
  assert_eq!(
    key("acme-api:src/users/users.service.ts:16:20"),
    "ts:acme-api:acme-api:src/users/users.service::UsersService::users#field"
  );
  assert_eq!(key("src/users/users.service.ts:20"), USERS_FIND_ONE);
  assert_eq!(key("acme-sdk:src/money.ts"), "file:acme-sdk:src/money.ts");
  assert_eq!(key("UsersService/findOne"), USERS_FIND_ONE);
  assert_eq!(
    key("PgInvoiceStore/find"),
    "rust:acme-billing:acme-billing:store::PgInvoiceStore::<InvoiceStore>::find#method"
  );
  assert_eq!(
    key("<PgInvoiceStore as InvoiceStore>::find"),
    "rust:acme-billing:acme-billing:store::PgInvoiceStore::<InvoiceStore>::find#method"
  );
  assert!(matches!(fixture.query.resolve_target("findOne").unwrap(), Targeted::Ambiguous { candidates, .. } if candidates.len() == 4));
  assert_eq!(
    fixture.query.resolve_target("acme-api:src/users/users.service.ts:2").unwrap(),
    Targeted::NotFound {
      spec: "acme-api:src/users/users.service.ts:2".to_owned(),
      hint: "no symbol encloses acme-api:src/users/users.service.ts:2".to_owned()
    }
  );
  assert!(matches!(fixture.query.resolve_target("GET /users/:id"), Err(QueryError::Invalid(_))));
}

#[test]
fn should_find_the_innermost_symbol_at_a_position() {
  let fixture = common::acme();
  fixture.query.fresh().unwrap();
  let repo = RepoName::new("acme-api").unwrap();
  let path = RelPath::new("src/users/users.service.ts").unwrap();
  let at = |line, column| fixture.query.innermost_symbol(&repo, &path, line, column).unwrap().map(|entity| entity.key.to_string());

  assert_eq!(at(19, 5).as_deref(), Some(USERS_FIND_ONE));
  assert_eq!(at(16, 20).as_deref(), Some("ts:acme-api:acme-api:src/users/users.service::UsersService::users#field"));
  assert_eq!(at(16, 3).as_deref(), Some("ts:acme-api:acme-api:src/users/users.service::UsersService::constructor#method"));
  assert_eq!(at(1, 1), None);
}

#[test]
fn should_read_the_declared_name_span_and_refuse_a_changed_file() {
  let fixture = common::acme();
  fixture.query.fresh().unwrap();
  let entity = found(fixture.query.resolve_target(USERS_FIND_ONE).unwrap());
  let file = found(fixture.query.resolve_target("acme-api:src/users/users.service.ts").unwrap());

  let NameSpan::Found { span } = fixture.query.name_span(&entity).unwrap() else {
    panic!()
  };
  assert_eq!((span.start_line, span.start_col, span.end_line, span.end_col), (18, 8, 18, 15));
  assert!(matches!(fixture.query.name_span(&file).unwrap(), NameSpan::Unavailable { .. }));

  let service = fixture.repo_root("acme-api").join("src/users/users.service.ts");
  let mut content = std::fs::read_to_string(&service).unwrap();
  content.push('\n');
  std::fs::write(&service, content).unwrap();
  assert_eq!(fixture.query.name_span(&entity).unwrap(), NameSpan::SourceChanged);
}

#[test]
fn should_serialize_graph_references_with_columns_and_tags() {
  let fixture = common::acme();

  let response = fixture.query.symbol_references(&ReferencesRequest::new("toCents")).unwrap();

  let json: serde_json::Value = serde_json::from_str(&render_json(&response).unwrap()).unwrap();
  let row = &json["answer"]["references"][7];
  assert_eq!(
    row["location"],
    serde_json::json!({ "repo": "acme-web", "path": "src/components/OrderForm.tsx", "line": 15, "column": 44 })
  );
  assert_eq!(row["side"], "inbound");
  assert_eq!(row["edge_kind"], "calls");
  assert_eq!(row["certainty"]["tag"], "[resolved]");
  assert_eq!(row["enclosing"]["key"], "ts:acme-web:acme-web:src/components/OrderForm::OrderForm#function");
  assert_eq!(json["answer"]["source"], "graph");
}

mod adversarial {
  use super::*;

  fn fixture() -> common::Fixture {
    common::fixture("adversarial", &["trait-dispatch-rust", "duplicate-names-rust"])
  }

  #[test]
  fn should_skip_trait_segments_unless_named_and_show_them_in_the_name_path() {
    let fixture = fixture();

    let skipped = fixture.query.find(&FindRequest::new("MemStore/get")).unwrap();
    let named = fixture.query.find(&FindRequest::new("<Store>/get")).unwrap();
    let all = fixture.query.find(&FindRequest::new("get")).unwrap();

    assert_eq!(
      fixture.text(&skipped).lines().skip(1).collect::<Vec<_>>(),
      vec![
        "find \"MemStore/get\": 1 match",
        "  MemStore/<Store>/get method trait-dispatch-rust:src/lib.rs:28  fn get(&self, key: &str) -> Option<String>",
      ]
    );
    assert_eq!(named.answer.matches.total, 2);
    assert_eq!(
      all.answer.matches.items.iter().map(|found| found.name_path.as_str()).collect::<Vec<_>>(),
      vec!["Store/get", "MemStore/<Store>/get", "DiskStore/<Store>/get"]
    );
  }

  #[test]
  fn should_match_absolute_paths_from_the_top_of_the_file_and_index_overloads_per_sibling_group() {
    let fixture = fixture();

    let absolute = fixture.query.find(&FindRequest::new("/glob_explicit/explicit_beats_glob")).unwrap();
    let relative_module = fixture.query.find(&FindRequest::new("consumer/glob_explicit/explicit_beats_glob")).unwrap();
    let not_from_top = fixture.query.find(&FindRequest::new("/explicit_beats_glob")).unwrap();
    let first = fixture.query.find(&FindRequest::new("parse[0]")).unwrap();
    let second = fixture.query.find(&FindRequest::new("parse[1]")).unwrap();

    assert_eq!(absolute.answer.matches.items[0].name_path, "glob_explicit/explicit_beats_glob");
    assert_eq!(relative_module.answer.matches.total, 1);
    assert_eq!(not_from_top.answer.matches.total, 0);
    assert_eq!(first.answer.matches.total, 2);
    assert_eq!(second.answer.matches.total, 0);
    assert!(matches!(fixture.query.resolve_target("parse").unwrap(), Targeted::Ambiguous { candidates, .. } if candidates.len() == 2));
  }

  #[test]
  fn should_show_the_store_trait_hierarchy_for_the_trait_and_its_method() {
    let fixture = fixture();

    let response = fixture.query.hierarchy(&HierarchyRequest::new("Store")).unwrap();
    let method = found(fixture.query.hierarchy(&HierarchyRequest::new("Store/get")).unwrap().answer);

    assert_eq!(
      fixture.text(&response).lines().skip(1).collect::<Vec<_>>(),
      vec![
        "hierarchy Store trait trait-dispatch-rust:src/lib.rs:3",
        "supertypes:",
        "  (none)",
        "subtypes:",
        "  implemented by MemStore struct trait-dispatch-rust:src/lib.rs:8 [resolved]",
        "  implemented by DiskStore struct trait-dispatch-rust:src/lib.rs:13 [resolved]",
      ]
    );
    assert_eq!(
      method.subtypes.iter().map(|node| node.entity.key.as_str()).collect::<Vec<_>>(),
      vec![
        "rust:trait-dispatch-rust:trait-dispatch-rust:::MemStore::<Store>::get#method",
        "rust:trait-dispatch-rust:trait-dispatch-rust:::DiskStore::<Store>::get#method",
      ]
    );
  }

  #[test]
  fn should_list_the_implemented_trait_method_and_tag_heuristic_dispatch_calls() {
    let fixture = fixture();

    let answer = found(fixture.query.symbol_references(&ReferencesRequest::new("MemStore/get")).unwrap().answer);

    assert_eq!(
      rows(&answer),
      vec![
        "trait-dispatch-rust:src/lib.rs:4:6 Store::get implemented by [resolved]",
        "trait-dispatch-rust:src/lib.rs:48:9 read_generic possibly calls [heuristic 0.30]",
        "trait-dispatch-rust:src/lib.rs:52:9 read_dyn possibly calls [heuristic 0.30]",
        "trait-dispatch-rust:src/lib.rs:56:9 read_impl possibly calls [heuristic 0.30]",
        "trait-dispatch-rust:src/lib.rs:60:9 read_mem calls [resolved]",
      ]
    );
    assert_eq!(answer.references[0].side, ReferenceSide::Implemented);
  }
}

mod overloads {
  use super::*;

  const TS: &str = "export function pick(a: string): string;\n\
                    export function pick(a: number): number;\n\
                    export function pick(a: any): any {\n  return a;\n}\n\n\
                    export class Box {\n  get get(): number { return 1; }\n  pick(): number { return pick(1); }\n}\n";
  const RS: &str = "#[cfg(unix)]\npub fn os() -> &'static str { \"unix\" }\n#[cfg(not(unix))]\npub fn os() -> &'static str { \"other\" }\n\n\
                    pub fn r#type() -> u8 { 1 }\n\npub fn call() -> u8 { r#type() }\n\n\
                    mod tests {\n  pub fn helper() -> u8 { super::call() }\n  mod deeper {\n    pub fn helper() {}\n  }\n}\n";

  fn fixture() -> common::Fixture {
    common::single_repo(
      "demo",
      &[
        ("package.json", r#"{"name":"demo","version":"1.0.0"}"#),
        ("Cargo.toml", "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        ("src/over.ts", TS),
        ("src/lib.rs", RS),
      ],
    )
  }

  fn lines(fixture: &common::Fixture, pattern: &str) -> Vec<String> {
    let response = fixture.query.find(&FindRequest::new(pattern)).unwrap();
    response
      .answer
      .matches
      .items
      .iter()
      .map(|found| format!("{} {}", found.name_path, found.entity.location.as_ref().map(ToString::to_string).unwrap_or_default()))
      .collect()
  }

  #[test]
  fn should_select_overloads_and_cfg_duplicates_by_source_order() {
    let fixture = fixture();

    assert_eq!(lines(&fixture, "pick[1]"), vec!["pick demo:src/over.ts:2"]);
    assert_eq!(lines(&fixture, "/pick[2]"), vec!["pick demo:src/over.ts:3"]);
    assert_eq!(lines(&fixture, "pick[0]"), vec!["pick demo:src/over.ts:1", "Box/pick demo:src/over.ts:9"]);
    assert_eq!(lines(&fixture, "os[1]"), vec!["os demo:src/lib.rs:3"]);
    assert_eq!(lines(&fixture, "/tests/deeper/helper"), vec!["tests/deeper/helper demo:src/lib.rs:13"]);
    assert_eq!(lines(&fixture, "helper"), vec!["tests/helper demo:src/lib.rs:11", "tests/deeper/helper demo:src/lib.rs:13"]);
  }

  #[test]
  fn should_list_the_union_of_overloads_for_a_bare_overloaded_name() {
    let fixture = common::single_repo(
      "demo",
      &[
        ("package.json", r#"{"name":"demo","version":"1.0.0"}"#),
        (
          "src/over.ts",
          "export function pick(a: string): string;\nexport function pick(a: number): number;\nexport function pick(a: any): any {\n  return a;\n}\nexport const x = pick(1);\n",
        ),
      ],
    );

    let union = found(fixture.query.symbol_references(&ReferencesRequest::new("pick")).unwrap().answer);
    let single = found(fixture.query.symbol_references(&ReferencesRequest::new("pick[2]")).unwrap().answer);

    assert_eq!(
      union
        .overloads
        .iter()
        .map(|overload| overload.location.as_ref().and_then(|location| location.line))
        .collect::<Vec<_>>(),
      vec![Some(1), Some(2), Some(3)]
    );
    assert_eq!(rows(&union), vec!["demo:src/over.ts:6:18 x calls [resolved]"]);
    assert_eq!(union.references[0].overload, Some(2));
    assert!(single.overloads.is_empty());
    assert_eq!(single.references[0].overload, None);
    assert_eq!(rows(&single), vec!["demo:src/over.ts:6:18 x calls [resolved]"]);
  }

  #[test]
  fn should_outline_nested_rust_modules_to_the_requested_depth() {
    let fixture = fixture();

    let response = fixture
      .query
      .outline(&OutlineRequest {
        file: "src/lib.rs".to_owned(),
        depth: 3,
      })
      .unwrap();

    assert_eq!(
      fixture.text(&response).lines().skip(1).collect::<Vec<_>>(),
      vec![
        "outline demo:src/lib.rs source=graph (from the index, syntactic; for the exact language-server outline use source=lsp)",
        "  os function :1  pub fn os() -> &'static str",
        "  os function :3  pub fn os() -> &'static str",
        "  type function :6  pub fn r#type() -> u8",
        "  call function :8  pub fn call() -> u8",
        "  tests module :10  mod tests",
        "    helper function :11  pub fn helper() -> u8",
        "    deeper module :12  mod deeper",
        "      helper function :13  pub fn helper()",
      ]
    );
  }

  #[test]
  fn should_locate_raw_identifiers_and_getters_named_like_keywords() {
    let fixture = fixture();
    fixture.query.fresh().unwrap();
    let raw = found(fixture.query.resolve_target("type").unwrap());
    let getter = found(fixture.query.resolve_target("Box/get").unwrap());

    let NameSpan::Found { span: raw_span } = fixture.query.name_span(&raw).unwrap() else {
      panic!()
    };
    let NameSpan::Found { span: getter_span } = fixture.query.name_span(&getter).unwrap() else {
      panic!()
    };
    let references = fixture.query.graph_references(&raw).unwrap();

    assert_eq!((raw_span.start_line, raw_span.start_col, raw_span.end_col), (6, 7, 13));
    assert_eq!((getter_span.start_line, getter_span.start_col, getter_span.end_col), (8, 6, 9));
    assert_eq!(rows(&references), vec!["demo:src/lib.rs:8:23 call calls [resolved]"]);
  }
}

#[test]
fn should_locate_a_type_level_implementation_on_the_implementing_type_name() {
  let fixture = common::acme();

  let answer = found(fixture.query.symbol_references(&ReferencesRequest::new("PgInvoiceStore")).unwrap().answer);

  let heritage: Vec<_> = answer.references.iter().filter(|reference| reference.side == ReferenceSide::Heritage).collect();
  assert_eq!(heritage.len(), 1);
  assert_eq!(heritage[0].location.as_ref().map(ToString::to_string).as_deref(), Some("acme-billing:src/store.rs:35:23"));
  assert_eq!(heritage[0].phrase, "implements");
  assert_eq!(heritage[0].other.key, "rust:acme-billing:acme-billing:store::InvoiceStore#trait");
  assert!(
    answer
      .references
      .iter()
      .all(|reference| reference.location.as_ref().is_none_or(|location| location.line != Some(15)))
  );
  assert!(answer.references.iter().all(|reference| reference.side != ReferenceSide::Implemented));
}

#[test]
fn should_keep_one_row_per_name_token_preferring_the_specific_edge() {
  let fixture = common::acme();

  let answer = found(fixture.query.symbol_references(&ReferencesRequest::new("InvoiceStore")).unwrap().answer);

  let at_impl: Vec<_> = answer
    .references
    .iter()
    .filter(|reference| reference.location.as_ref().map(ToString::to_string).as_deref() == Some("acme-billing:src/store.rs:35:6"))
    .collect();
  assert_eq!(at_impl.len(), 1);
  assert_eq!(at_impl[0].edge_kind, "implements");
  assert_eq!(at_impl[0].other.key, "rust:acme-billing:acme-billing:store::PgInvoiceStore#struct");
  assert_eq!(at_impl[0].edge_ids.len(), 2);
  assert_eq!(answer.references.len(), 12);
}

#[test]
fn should_anchor_rust_crate_prefixes_at_the_crate_root() {
  let fixture = common::acme();
  fixture.query.fresh().unwrap();
  let trait_find = "rust:acme-billing:acme-billing:store::InvoiceStore::find#method";

  for text in ["crate::store::InvoiceStore::find", "acme_billing::store::InvoiceStore/find", "store::InvoiceStore/find"] {
    let entity = found(fixture.query.resolve_target(text).unwrap());
    assert_eq!(entity.key.to_string(), trait_find, "{text}");
  }
  assert!(matches!(fixture.query.resolve_target("crate::InvoiceStore::find").unwrap(), Targeted::NotFound { .. }));
}

#[test]
fn should_answer_an_unreadable_artifact_with_unavailable_and_line_only_rows() {
  let fixture = common::fixture("adversarial", &["trait-dispatch-rust"]);
  fixture.query.fresh().unwrap();
  let declaration = found(fixture.query.resolve_target("Store/get").unwrap());
  let implementation = found(fixture.query.resolve_target("MemStore/get").unwrap());
  fixture
    .query
    .engine()
    .store()
    .write(|tx| tx.put_blob("artifact:trait-dispatch-rust:src/lib.rs", 1, b"not an artifact"))
    .unwrap();

  let name = fixture.query.name_span(&declaration).unwrap();
  let answer = fixture.query.graph_references(&implementation).unwrap();

  assert!(matches!(name, NameSpan::Unavailable { .. }), "{name:?}");
  assert_eq!(rows(&answer)[0], "trait-dispatch-rust:src/lib.rs:4 Store::get implemented by [resolved]");
}

#[test]
fn should_refuse_only_a_bare_endpoint_as_a_target() {
  let fixture = common::acme();

  assert!(matches!(fixture.query.resolve_target("GET /users/:id"), Err(QueryError::Invalid(_))));
  assert!(matches!(
    fixture.query.resolve_target("GET /users/:id should return 404").unwrap(),
    Targeted::NotFound { .. }
  ));
}

#[test]
fn should_hint_that_a_missing_name_looks_macro_generated() {
  let fixture = common::fixture("adversarial", &["macro-heavy-rust"]);

  let response = fixture.query.symbol_references(&ReferencesRequest::new("add")).unwrap();

  assert_eq!(
    response.answer,
    Targeted::NotFound {
      spec: "add".to_owned(),
      hint: "no entity matches add; `add` looks macro-generated (1 macro call site, e.g. macro-heavy-rust:src/lib.rs:34); macro-generated items have no symbol: search for the macro definition".to_owned()
    }
  );
}

mod heritage {
  use super::*;

  const SHAPES: &str = "export interface Shape {}\n\
                        export class Root {}\n\
                        export class Base extends Root {}\n\
                        export class Crate extends Base implements Shape {}\n\
                        export class Other extends Crate {}\n\
                        export interface Pair {}\n\
                        export class Pair {}\n";
  const SPEC: &str = "describe('GET /users/:id', () => {\n  it('should round 1.5 up', () => {});\n});\n";

  fn fixture() -> common::Fixture {
    common::single_repo(
      "shapes",
      &[
        ("package.json", r#"{"name":"shapes","version":"1.0.0"}"#),
        ("src/shapes.ts", SHAPES),
        ("src/shapes.spec.ts", SPEC),
      ],
    )
  }

  #[test]
  fn should_not_report_a_ts_class_declaration_as_its_own_heritage_site() {
    let fixture = fixture();

    let answer = found(fixture.query.symbol_references(&ReferencesRequest::new("Crate")).unwrap().answer);

    assert_eq!(rows(&answer), vec!["shapes:src/shapes.ts:5:28 Other extends [resolved]"]);
    assert_eq!(answer.references[0].side, ReferenceSide::Inbound);
  }

  #[test]
  fn should_note_when_the_depth_limit_cuts_the_hierarchy() {
    let fixture = fixture();
    let mut shallow = HierarchyRequest::new("Crate");
    shallow.depth = 1;

    let cut = found(fixture.query.hierarchy(&shallow).unwrap().answer);
    let full = found(fixture.query.hierarchy(&HierarchyRequest::new("Crate")).unwrap().answer);

    assert_eq!(
      cut.caveats,
      vec![atlas_query::Caveat::Note {
        message: "the hierarchy continues past depth 1; raise depth to see more".to_owned()
      }]
    );
    assert!(full.caveats.is_empty());
    assert_eq!(
      full.supertypes.iter().map(|node| (node.entity.short.as_str(), node.children.len())).collect::<Vec<_>>(),
      vec![("Shape", 0), ("Base", 1)]
    );
  }

  #[test]
  fn should_group_overload_indexes_by_kind() {
    let fixture = fixture();

    let first = fixture.query.find(&FindRequest::new("Pair[0]")).unwrap();
    let second = fixture.query.find(&FindRequest::new("Pair[1]")).unwrap();

    assert_eq!(first.answer.matches.total, 2);
    assert_eq!(second.answer.matches.total, 0);
  }

  #[test]
  fn should_retry_a_split_test_name_as_one_segment() {
    let fixture = fixture();

    let dotted = fixture.query.find(&FindRequest::new("should round 1.5 up")).unwrap();
    let slashed = fixture.query.find(&FindRequest::new("GET /users/:id")).unwrap();

    assert_eq!(dotted.answer.matches.items.iter().map(|found| found.entity.kind.as_str()).collect::<Vec<_>>(), vec!["test"]);
    assert_eq!(
      slashed.answer.matches.items.iter().map(|found| found.entity.kind.as_str()).collect::<Vec<_>>(),
      vec!["test_suite"]
    );
  }
}

mod renamed_method {
  use super::*;

  fn fixture() -> common::Fixture {
    common::single_repo(
      "users",
      &[
        ("package.json", r#"{"name":"users","version":"1.0.0"}"#),
        (
          "src/users.service.ts",
          "export class UsersService {\n  findById(id: string): string {\n    return id;\n  }\n}\n",
        ),
        (
          "src/users.service.spec.ts",
          "import { UsersService } from './users.service';\n\ndescribe('UsersService.findOne', () => {\n  it('should find', () => {\n    new UsersService().findById('u1');\n  });\n});\n",
        ),
      ],
    )
  }

  #[test]
  fn should_not_resolve_a_code_looking_path_to_an_exactly_named_test_block() {
    let fixture = fixture();
    fixture.query.fresh().unwrap();
    let expected_hint = "no entity matches UsersService.findOne; try a search for UsersService.findOne; \
                         test_suite ts:users:users:src/users.service.spec::UsersService.findOne#test_suite is named exactly \"UsersService.findOne\", pass its key to target it";

    let resolved = fixture.query.resolve_target("UsersService.findOne").unwrap();
    let references = fixture.query.symbol_references(&ReferencesRequest::new("UsersService.findOne")).unwrap();
    let searched = fixture.query.find(&FindRequest::new("UsersService.findOne")).unwrap();

    assert_eq!(
      resolved,
      Targeted::NotFound {
        spec: "UsersService.findOne".to_owned(),
        hint: expected_hint.to_owned()
      }
    );
    assert_eq!(
      references.answer,
      Targeted::NotFound {
        spec: "UsersService.findOne".to_owned(),
        hint: expected_hint.to_owned()
      }
    );
    assert_eq!(
      searched.answer.matches.items.iter().map(|found| found.entity.key.as_str()).collect::<Vec<_>>(),
      vec!["ts:users:users:src/users.service.spec::UsersService.findOne#test_suite"]
    );
  }
}

mod nested_describe {
  use super::*;

  const SPEC: &str = "import { UsersService } from './users.service';\n\n\
                      describe('UsersService', () => {\n  describe('findOne', () => {\n    it('should find one', () => {\n      new UsersService().findById('u1');\n    });\n  });\n});\n\n\
                      describe('loadAll', () => {});\n";

  fn fixture() -> common::Fixture {
    common::single_repo(
      "users",
      &[
        ("package.json", r#"{"name":"users","version":"1.0.0"}"#),
        (
          "src/users.service.ts",
          "export class UsersService {\n  findById(id: string): string {\n    return id;\n  }\n}\n",
        ),
        ("src/users.service.spec.ts", SPEC),
      ],
    )
  }

  fn not_found(text: &str, name: &str, key: &str) -> Targeted<atlas_core::Entity> {
    Targeted::NotFound {
      spec: text.to_owned(),
      hint: format!("no entity matches {text}; try a search for {text}; test_suite {key} is named exactly {name:?}, pass its key to target it"),
    }
  }

  #[test]
  fn should_not_resolve_a_stale_code_name_to_a_nested_describe_block() {
    let fixture = fixture();
    fixture.query.fresh().unwrap();
    let suite = "ts:users:users:src/users.service.spec::UsersService::findOne#test_suite";

    for text in ["UsersService/findOne", "UsersService.findOne", "findOne"] {
      assert_eq!(fixture.query.resolve_target(text).unwrap(), not_found(text, "findOne", suite), "{text}");
    }
    assert_eq!(
      fixture.query.resolve_target("loadAll").unwrap(),
      not_found("loadAll", "loadAll", "ts:users:users:src/users.service.spec::loadAll#test_suite")
    );
    assert!(matches!(
      fixture.query.symbol_references(&ReferencesRequest::new("UsersService/findOne")).unwrap().answer,
      Targeted::NotFound { .. }
    ));
    assert_eq!(found(fixture.query.resolve_target(suite).unwrap()).key.to_string(), suite);
  }

  #[test]
  fn should_still_list_describe_blocks_in_find_and_resolve_whitespace_test_names() {
    let fixture = fixture();
    fixture.query.fresh().unwrap();

    let listed = fixture.query.find(&FindRequest::new("findOne")).unwrap();
    let test = found(fixture.query.resolve_target("should find one").unwrap());

    assert_eq!(
      listed.answer.matches.items.iter().map(|found| found.entity.key.as_str()).collect::<Vec<_>>(),
      vec!["ts:users:users:src/users.service.spec::UsersService::findOne#test_suite"]
    );
    assert_eq!(test.key.to_string(), "ts:users:users:src/users.service.spec::UsersService::findOne::should find one#test");
  }
}
