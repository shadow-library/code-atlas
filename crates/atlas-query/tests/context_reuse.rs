mod common;

use atlas_query::{ContextAnswer, ContextRequest};

fn context(task: &str) -> ContextAnswer {
  common::acme().query.context(&ContextRequest::new(task)).unwrap().answer
}

#[test]
fn should_seed_a_code_span_type_that_the_named_repository_disambiguates() {
  let answer = context("In acme-api, add a nullable `deletedAt` timestamp to the `User` entity");

  let user = answer
    .must_read
    .iter()
    .find(|item| item.entity.short == "User")
    .map(|item| (item.entity.key.as_str(), item.why.as_str()));
  assert_eq!(
    user,
    Some((
      "ts:acme-api:acme-api:src/users/user.entity::User#class",
      "named in task as User (the only match in acme-api)"
    ))
  );
}

#[test]
fn should_not_seed_a_project_type_named_like_a_platform_type_unless_the_scope_narrows_it() {
  let fixture = common::single_repo(
    "web",
    &[
      ("package.json", r#"{ "name": "web" }"#),
      ("src/client/types.ts", "export interface Response {\n  status: number;\n  body: string;\n}\n"),
      (
        "src/static.ts",
        "export function serveFile(path: string): number {\n  return path.length > 0 ? 200 : 404;\n}\n",
      ),
    ],
  );
  let seeded = |task: &str| -> Vec<String> {
    let answer = fixture.query.context(&ContextRequest::new(task)).unwrap().answer;
    answer
      .must_read
      .iter()
      .filter(|item| item.entity.short == "Response" && item.why.starts_with("named in task"))
      .map(|item| item.why.clone())
      .collect()
  };

  assert_eq!(seeded("serveFile should answer with a `Response` with status 304"), Vec::<String>::new());
  assert_eq!(
    seeded("In web, return the `Response` interface from serveFile"),
    ["named in task as Response (the only match in web)"]
  );
}

const LINES: &str = "export interface Line {\n  quantity: number;\n  unitPrice: number;\n  discount?: number;\n}\n";

fn totals(name: &str, line_type: &str) -> String {
  format!(
    "import {{ type Line }} from './line';\n\nexport function {name}(lines: {line_type}[]): number {{\n  let sum = 0;\n  for (const line of lines) {{\n    const gross = line.quantity * line.unitPrice;\n    const discount = line.discount ?? 0;\n    sum += gross - discount;\n  }}\n  if (sum < 0) {{\n    throw new Error('negative total');\n  }}\n  return Math.round(sum * 100) / 100;\n}}\n"
  )
}

#[test]
fn should_say_whether_a_structural_reuse_candidate_is_importable_from_the_seed() {
  let invoices = totals("computeInvoiceTotal", "Line");
  let orders = totals("sumOrderLines", "Line");
  let cart = totals("tallyCartLines", "Line").replace("export function", "function");
  let fixture = common::single_repo(
    "web",
    &[
      ("package.json", r#"{ "name": "web" }"#),
      ("src/line.ts", LINES),
      ("src/invoices.ts", invoices.as_str()),
      ("src/orders.ts", orders.as_str()),
      ("src/cart.ts", cart.as_str()),
    ],
  );

  let answer = fixture.query.context(&ContextRequest::new("change computeInvoiceTotal to round half up")).unwrap().answer;

  let reuse: Vec<(&str, &str)> = answer.reuse.iter().map(|item| (item.entity.short.as_str(), item.why.as_str())).collect();
  assert_eq!(
    reuse,
    [
      (
        "tallyCartLines",
        "same shape as computeInvoiceTotal [structural 1.00]; not exported in web:src/cart.ts, same package as the likely change site; behaviour unverified"
      ),
      (
        "sumOrderLines",
        "same shape as computeInvoiceTotal [structural 1.00]; exported from web (likely change site); behaviour unverified"
      ),
    ]
  );
}
