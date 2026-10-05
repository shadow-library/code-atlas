mod common;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use atlas_ide::{DeclarationRequest, LaunchSpec, OutlineRequest, ReferencesRequest};
use atlas_lsp::{Launch, ServerCommand, ServerKind};
use common::*;

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
  let Some(home) = env::var_os("HOME") else { return Vec::new() };
  let Ok(caches) = fs::read_dir(Path::new(&home).join(".npm/_npx")) else {
    return Vec::new();
  };
  caches
    .flatten()
    .map(|cache| cache.path().join("node_modules/typescript-language-server/lib/cli.mjs"))
    .filter(|entry| entry.is_file())
    .collect()
}

fn typescript_language_server() -> Result<ServerCommand, String> {
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

fn rust_analyzer() -> Result<PathBuf, String> {
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

fn body(text: &str) -> String {
  text.lines().skip(1).map(|line| format!("{line}\n")).collect()
}

fn head(text: &str) -> &str {
  text.lines().next().unwrap_or_default()
}

fn typescript(test: &str, group: &str, repos: &[&str]) -> Option<Fixture> {
  let command = match typescript_language_server() {
    Ok(command) => command,
    Err(reason) => {
      eprintln!("SKIPPED {test}: typescript-language-server unavailable ({reason})");
      return None;
    }
  };
  let fixture = Fixture::copied(group, repos);
  fixture.launcher.set(
    ServerKind::TypeScript,
    LaunchSpec {
      tsserver: None,
      ..launch(ServerKind::TypeScript, command)
    },
  );
  Some(fixture)
}

#[test]
fn should_find_users_service_find_one_references_with_typescript_language_server() {
  let Some(fixture) = typescript(
    "should_find_users_service_find_one_references_with_typescript_language_server",
    "acme",
    &["acme-api", "acme-sdk", "acme-web"],
  ) else {
    return;
  };

  let references = fixture.ide.references(&ReferencesRequest::new("UsersService.findOne")).unwrap();
  let outline = fixture.ide.outline(&OutlineRequest::new("src/users/users.service.ts")).unwrap();
  let declaration = fixture
    .ide
    .declaration(&DeclarationRequest::Regex {
      path: "src/users/users.controller.ts".to_owned(),
      regex: r"usersService\.(findOne)\(".to_owned(),
    })
    .unwrap();

  let to_cents = text(&fixture.ide.references(&ReferencesRequest::new("toCents")).unwrap());
  let orders = text(&fixture.ide.references(&ReferencesRequest::new("OrdersService.findOne")).unwrap());
  assert_eq!(
    orders.lines().skip(1).collect::<Vec<_>>(),
    vec![
      "acme-api (1)",
      "  OrdersController.findOne  src/orders/orders.controller.ts:16:31  return this.ordersService.findOne(id);",
      "outside the language server's view:",
      "  mock (replaced) [heuristic 0.60]  acme-api:src/orders/orders.controller.spec.ts:5:27  OrdersController  const ordersService = { findOne: jest.fn(), create: jest.fn() };",
      "  mock (replaced) [heuristic 0.60]  acme-api:src/orders/orders.controller.spec.ts:11:19  OrdersController.should delegate findOne to the service  ordersService.findOne.mockResolvedValue({ id: 'o1', display: '$12.00' });",
      "  mock (replaced) [heuristic 0.60]  acme-api:src/orders/orders.controller.spec.ts:13:26  OrdersController.should delegate findOne to the service  expect(ordersService.findOne).toHaveBeenCalledWith('o1');",
    ],
    "{orders}"
  );
  let (references, outline, declaration) = (text(&references), text(&outline), text(&declaration));
  let outside: Vec<&str> = to_cents.lines().skip_while(|line| !line.starts_with("outside")).take(3).collect();
  assert_eq!(
    outside,
    vec![
      "outside the language server's view:",
      "  other repo [heuristic 0.85]  acme-web:src/components/OrderForm.tsx:1:10  src/components/OrderForm.tsx  import { toCents } from '@acme/sdk';",
      "  other repo [heuristic 0.85]  acme-web:src/components/OrderForm.tsx:15:44  OrderForm  submit({ userId: me.id, subtotalCents: toCents(amount) });",
    ],
    "{to_cents}"
  );
  assert!(
    head(&references).starts_with("references UsersService.findOne method acme-api:src/users/users.service.ts:18:9  (typescript-language-server "),
    "{references}"
  );
  assert!(head(&references).ends_with(") [compiler]"), "{references}");
  assert_eq!(
    body(&references),
    concat!(
      "acme-api (4)\n",
      "  UsersController.findOne  src/users/users.controller.ts:16:30  return this.usersService.findOne(id);\n",
      "  UsersService.findOne.should return the user when it exists (test)  src/users/users.service.spec.ts:15:28  await expect(service.findOne('u1')).resolves.toMatchObject({ id: 'u1' });\n",
      "  UsersService.findOne.should throw NotFoundException when the user is missing (test)  src/users/users.service.spec.ts:20:28  await expect(service.findOne('missing')).rejects.toBeInstanceOf(NotFoundException);\n",
      "  UsersService.update  src/users/users.service.ts:29:29  const user = await this.findOne(id);\n",
      "outside the language server's view:\n",
      "  (none)\n",
    )
  );
  assert_eq!(
    body(&outline),
    concat!(
      "  CreateUserInput interface 7:18\n",
      "    email property 8:3\n",
      "    displayName property 9:3\n",
      "  UpdateUserInput variable 12:13\n",
      "  UsersService class 15:14\n",
      "    constructor constructor 16:3\n",
      "    users property 16:56\n",
      "    findOne method 18:9\n",
      "    create method 24:3\n",
      "    update method 28:9\n",
      "    remove method 33:9\n",
    )
  );
  assert!(
    head(&declaration).starts_with("declaration at acme-api:src/users/users.controller.ts:16:30  (typescript-language-server "),
    "{declaration}"
  );
  assert_eq!(
    body(&declaration),
    "  definition  UsersService.findOne  acme-api:src/users/users.service.ts:18:9  async findOne(id: string): Promise<User> {\n"
  );
}

#[test]
fn should_list_payment_provider_charge_implementations_with_typescript_language_server() {
  let Some(fixture) = typescript(
    "should_list_payment_provider_charge_implementations_with_typescript_language_server",
    "adversarial",
    &["ts-overrides"],
  ) else {
    return;
  };

  let implementations = fixture.ide.implementations("PaymentProvider.charge").unwrap();

  let implementations = text(&implementations);
  assert!(
    head(&implementations).starts_with("implementations of PaymentProvider.charge method ts-overrides:src/payment-provider.ts:8:3  ("),
    "{implementations}"
  );
  assert_eq!(
    body(&implementations),
    concat!(
      "  ts-overrides  PaypalProvider.charge  src/paypal-provider.ts:7:9  async charge(amountCents: number): Promise<ChargeResult> {\n",
      "  ts-overrides  StripeProvider.charge  src/stripe-provider.ts:7:9  async charge(amountCents: number): Promise<ChargeResult> {\n",
    )
  );
}

#[test]
fn should_find_store_get_references_with_rust_analyzer() {
  let binary = match rust_analyzer() {
    Ok(binary) => binary,
    Err(reason) => {
      eprintln!("SKIPPED should_find_store_get_references_with_rust_analyzer: rust-analyzer unavailable ({reason})");
      return;
    }
  };
  let fixture = Fixture::copied("adversarial", &["trait-dispatch-rust"]);
  fixture.launcher.set(
    ServerKind::RustAnalyzer,
    LaunchSpec {
      command: ServerCommand::new(binary),
      launch: Launch::Installed,
      server_version: "located".to_owned(),
      tsserver: None,
    },
  );

  let references = fixture.ide.references(&ReferencesRequest::new("Store::get")).unwrap();

  let references = text(&references);
  assert!(
    head(&references).starts_with("references Store::get method trait-dispatch-rust:src/lib.rs:4:6  (rust-analyzer "),
    "{references}"
  );
  let lines: Vec<&str> = references.lines().skip(1).take(9).collect();
  assert_eq!(
    lines,
    vec![
      "trait-dispatch-rust (6)",
      "  MemStore::<Store>::get  src/lib.rs:28:6  fn get(&self, key: &str) -> Option<String> {",
      "  DiskStore::<Store>::get  src/lib.rs:38:6  fn get(&self, key: &str) -> Option<String> {",
      "  read_generic  src/lib.rs:48:9  store.get(key)",
      "  read_dyn  src/lib.rs:52:9  store.get(key)",
      "  read_impl  src/lib.rs:56:9  store.get(key)",
      "  read_mem  src/lib.rs:60:9  store.get(key)",
      "outside the language server's view:",
      "  (none)",
    ],
    "{references}"
  );
}
