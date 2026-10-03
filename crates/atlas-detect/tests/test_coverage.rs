mod common;

use atlas_detect::TestDetector;
use common::{lines, render_repo, render_source};

#[test]
fn should_link_acme_spec_suites_to_their_sibling_sources() {
  assert_eq!(
    render_repo(&TestDetector, "acme/acme-api"),
    lines(&[
      "covers_file OrdersController -> file:acme-api:src/orders/orders.controller.ts [heuristic 0.60]",
      "covers_file UsersService -> file:acme-api:src/users/users.service.ts [heuristic 0.60]",
    ])
  );
  assert_eq!(
    render_repo(&TestDetector, "acme/acme-sdk"),
    lines(&[
      "covers_file AcmeClient -> file:acme-sdk:src/client.ts [heuristic 0.60]",
      "covers_file toCents -> file:acme-sdk:src/money.ts [heuristic 0.60]",
    ])
  );
}

#[test]
fn should_link_rust_inline_test_modules_and_integration_tests() {
  assert_eq!(
    render_repo(&TestDetector, "acme/acme-billing"),
    lines(&[
      "covers_file stored_invoice_is_returned -> file:acme-billing:src/lib.rs [heuristic 0.60]",
      "covers_file tests -> file:acme-billing:src/money.rs [syntactic]",
      "covers_file unknown_invoice_returns_404 -> file:acme-billing:src/lib.rs [heuristic 0.60]",
    ])
  );
}

#[test]
fn should_ignore_test_modules_without_a_cfg_test_gate_and_files_without_a_subject() {
  let rust = "mod tests {\n  #[test]\n  fn t() {}\n}\n";
  assert!(render_source(&TestDetector, "svc", "src/lib.rs", rust).is_empty());
  let ts = "describe('x', () => { it('should work', () => {}); });\n";
  assert!(render_source(&TestDetector, "web", "src/setup.ts", ts).is_empty());
  assert_eq!(
    render_source(&TestDetector, "svc", "src/parser_test.rs", "#[test]\nfn parses() {}\n"),
    lines(&["covers_file parses -> file:svc:src/parser.rs [heuristic 0.60]"])
  );
}
