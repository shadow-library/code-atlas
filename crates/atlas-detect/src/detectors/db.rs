use atlas_core::{ArgSummary, EdgeKind, EntityKey, EntityKind, FileFacts, LocalSymbolId, NodeRef, RefKind, ReferenceFact, Span, TemplatePart};
use serde_json::json;

use super::detector;
use crate::cert::Cert;
use crate::sink::{Sink, attrs};
use crate::view::FileView;

pub struct DbDetector;

detector!(DbDetector, "db", 1);

const SQL_CALLS: [&str; 10] = [
  "query",
  "execute",
  "sql",
  "raw",
  "queryRaw",
  "executeRaw",
  "$queryRaw",
  "$executeRaw",
  "query_as",
  "query_scalar",
];
const SQL_STATEMENTS: [&str; 7] = ["select", "insert", "update", "delete", "with", "merge", "replace"];
const PRISMA_READS: [&str; 8] = [
  "findMany",
  "findUnique",
  "findFirst",
  "findUniqueOrThrow",
  "findFirstOrThrow",
  "count",
  "aggregate",
  "groupBy",
];
const PRISMA_WRITES: [&str; 7] = ["create", "createMany", "update", "updateMany", "upsert", "delete", "deleteMany"];
const FROM_FUNCTIONS: [&str; 5] = ["extract", "substring", "trim", "position", "overlay"];
const SQL_RESERVED: [&str; 58] = [
  "select",
  "from",
  "where",
  "join",
  "inner",
  "outer",
  "left",
  "right",
  "full",
  "cross",
  "natural",
  "on",
  "using",
  "as",
  "and",
  "or",
  "not",
  "null",
  "is",
  "in",
  "exists",
  "set",
  "values",
  "value",
  "into",
  "update",
  "delete",
  "insert",
  "group",
  "order",
  "by",
  "having",
  "limit",
  "offset",
  "union",
  "intersect",
  "except",
  "all",
  "distinct",
  "case",
  "when",
  "then",
  "else",
  "end",
  "table",
  "lateral",
  "only",
  "unnest",
  "with",
  "returning",
  "default",
  "like",
  "between",
  "asc",
  "desc",
  "true",
  "false",
  "for",
];
const ENGLISH_STOPWORDS: [&str; 33] = [
  "the", "a", "an", "this", "that", "these", "those", "it", "its", "my", "our", "your", "their", "his", "her", "now", "here", "there", "some", "any", "each", "every", "me", "us",
  "them", "him", "you", "we", "they", "i", "to", "of", "at",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TableAccess {
  pub table: String,
  pub write: bool,
}

impl DbDetector {
  fn prefilter(&self, facts: &FileFacts) -> bool {
    facts.references.iter().any(|reference| {
      reference.path.last().is_some_and(|name| {
        SQL_CALLS.contains(&name.as_str()) || PRISMA_READS.contains(&name.as_str()) || PRISMA_WRITES.contains(&name.as_str()) || matches!(name.as_str(), "Entity" | "Table")
      })
    })
  }

  fn run(&self, view: &FileView<'_>, sink: &mut Sink) {
    self.models(view, sink);
    for reference in &view.facts.references {
      match reference.kind {
        RefKind::Call | RefKind::MethodCall | RefKind::MacroCall => {
          self.sql(view, sink, reference);
          self.prisma(view, sink, reference);
        }
        _ => {}
      }
    }
  }

  fn models(&self, view: &FileView<'_>, sink: &mut Sink) {
    for class in view.facts.symbols.iter().filter(|symbol| symbol.kind == EntityKind::Class) {
      for attribute in &class.attributes {
        let [local] = attribute.path.as_slice() else { continue };
        let (orm, table, cert) = if view.imports_named(local, &["typeorm"], "Entity") {
          match attribute.args.first() {
            Some(ArgSummary::Str(name)) => ("typeorm", name.clone(), Cert::Syntactic),
            Some(ArgSummary::Object(entries)) => match entries.iter().find(|entry| entry.key.static_name() == Some("name")).map(|entry| &entry.value) {
              Some(ArgSummary::Str(name)) => ("typeorm", name.clone(), Cert::Syntactic),
              _ => ("typeorm", snake_case(&class.name), Cert::heuristic(0.6, "typeorm-default-table-name")),
            },
            None => ("typeorm", snake_case(&class.name), Cert::heuristic(0.6, "typeorm-default-table-name")),
            Some(_) => continue,
          }
        } else if view.imports_named(local, &["sequelize-typescript"], "Table") {
          let Some(ArgSummary::Object(entries)) = attribute.args.first() else { continue };
          let Some(ArgSummary::Str(name)) = entries.iter().find(|entry| entry.key.static_name() == Some("tableName")).map(|entry| &entry.value) else {
            continue;
          };
          ("sequelize-typescript", name.clone(), Cert::Syntactic)
        } else {
          continue;
        };
        let key = EntityKey::DbTable { name: table };
        sink.entity(key.clone(), attribute.span, cert, attrs([("experimental", json!(true))]));
        sink.edge(
          EdgeKind::MapsToTable,
          NodeRef::Local(class.local),
          NodeRef::Key(key),
          attribute.span,
          cert,
          attrs([("orm", json!(orm))]),
        );
      }
    }
  }

  fn sql(&self, view: &FileView<'_>, sink: &mut Sink, reference: &ReferenceFact) {
    let Some(name) = reference.path.last() else { return };
    if !SQL_CALLS.contains(&name.as_str()) {
      return;
    }
    let Some(sql) = reference.args.iter().take(2).find_map(sql_text) else { return };
    for access in table_accesses(&sql) {
      emit_access(view, sink, reference.from, reference.span, access, Cert::heuristic(0.6, "sql-literal"));
    }
  }

  fn prisma(&self, view: &FileView<'_>, sink: &mut Sink, reference: &ReferenceFact) {
    if reference.kind != RefKind::MethodCall {
      return;
    }
    let [.., client, model, operation] = reference.path.as_slice() else { return };
    let write = if PRISMA_WRITES.contains(&operation.as_str()) {
      true
    } else if PRISMA_READS.contains(&operation.as_str()) {
      false
    } else {
      return;
    };
    if model.starts_with('$') || model.starts_with('(') || !is_prisma_client(view, reference, client) {
      return;
    }
    let access = TableAccess { table: model.clone(), write };
    emit_access(view, sink, reference.from, reference.span, access, Cert::heuristic(0.6, "prisma-model-call"));
  }
}

fn is_prisma_client(view: &FileView<'_>, reference: &ReferenceFact, client: &str) -> bool {
  if client.to_ascii_lowercase().contains("prisma") {
    return true;
  }
  let [this, field, ..] = reference.path.as_slice() else { return false };
  this == "this"
    && reference
      .from
      .and_then(|local| view.class_of(local))
      .and_then(|class| view.field_type(class, field))
      .is_some_and(|type_text| type_text == "PrismaClient")
}

fn emit_access(view: &FileView<'_>, sink: &mut Sink, from: Option<LocalSymbolId>, span: Span, access: TableAccess, cert: Cert) {
  let key = EntityKey::DbTable { name: access.table };
  let kind = if access.write { EdgeKind::WritesTable } else { EdgeKind::ReadsTable };
  sink.entity(key.clone(), span, cert, attrs([("experimental", json!(true))]));
  sink.edge(kind, view.node(from), NodeRef::Key(key), span, cert, attrs([("experimental", json!(true))]));
}

fn sql_text(arg: &ArgSummary) -> Option<String> {
  let text = match arg {
    ArgSummary::Str(value) => value.clone(),
    ArgSummary::Template(parts) => parts
      .iter()
      .map(|part| match part {
        TemplatePart::Lit(value) => value.as_str(),
        TemplatePart::Expr(_) => " ? ",
      })
      .collect(),
    _ => return None,
  };
  let statement = strip_leading_comments(&text);
  let first_word = statement.split(|c: char| !c.is_ascii_alphabetic()).next()?.to_ascii_lowercase();
  SQL_STATEMENTS.contains(&first_word.as_str()).then(|| statement.to_owned())
}

fn strip_leading_comments(text: &str) -> &str {
  let mut rest = text.trim_start();
  loop {
    if let Some(line) = rest.strip_prefix("--") {
      rest = line.split_once('\n').map_or("", |(_, after)| after).trim_start();
    } else if let Some(block) = rest.strip_prefix("/*") {
      rest = block.split_once("*/").map_or("", |(_, after)| after).trim_start();
    } else {
      return rest;
    }
  }
}

pub(crate) fn table_accesses(sql: &str) -> Vec<TableAccess> {
  let words = sql_words(strip_leading_comments(sql));
  let lower: Vec<String> = words.iter().map(|word| word.to_ascii_lowercase()).collect();
  if !statement_is_well_formed(&lower) {
    return Vec::new();
  }
  let mut accesses = Vec::new();
  let mut functions: Vec<&str> = Vec::new();
  for (index, word) in lower.iter().enumerate() {
    match word.as_str() {
      "(" => {
        functions.push(index.checked_sub(1).and_then(|previous| lower.get(previous)).map_or("", String::as_str));
        continue;
      }
      ")" => {
        functions.pop();
        continue;
      }
      _ => {}
    }
    let previous = index.checked_sub(1).and_then(|previous| lower.get(previous)).map_or("", String::as_str);
    let write = match word.as_str() {
      "from" if functions.last().is_some_and(|function| FROM_FUNCTIONS.contains(function)) => continue,
      "from" => previous == "delete",
      "join" => false,
      "into" if insert_target_is_well_formed(&lower, index + 1) => true,
      "update" if !matches!(previous, "for" | "do" | "key") && update_target_is_well_formed(&lower, index + 1) => true,
      _ => continue,
    };
    let Some(table) = words.get(index + 1).and_then(|next| table_name(next)) else {
      continue;
    };
    let access = TableAccess { table, write };
    if !accesses.contains(&access) {
      accesses.push(access);
    }
  }
  accesses
}

fn statement_is_well_formed(words: &[String]) -> bool {
  let Some(verb) = words.first() else { return false };
  match verb.as_str() {
    "select" => select_list_is_well_formed(words),
    "update" => update_target_is_well_formed(words, 1),
    "insert" | "replace" => words.get(1).is_some_and(|word| word == "into") && insert_target_is_well_formed(words, 2),
    "delete" => words.get(1).is_some_and(|word| word == "from") && words.get(2).is_some_and(|word| table_name(word).is_some()),
    _ => true,
  }
}

fn select_list_is_well_formed(words: &[String]) -> bool {
  let mut depth = 0i32;
  let mut item: Vec<&str> = Vec::new();
  let mut items = 0usize;
  for word in words.iter().skip(1) {
    match word.as_str() {
      "(" => depth += 1,
      ")" => depth -= 1,
      "from" | ";" if depth == 0 => break,
      "," if depth == 0 => {
        if !select_item_is_well_formed(&item) {
          return false;
        }
        items += 1;
        item.clear();
      }
      other if depth == 0 => item.push(other),
      _ => {}
    }
  }
  (item.is_empty() && items > 0) || select_item_is_well_formed(&item)
}

fn select_item_is_well_formed(item: &[&str]) -> bool {
  let item = match item.first() {
    Some(&"distinct" | &"all") => &item[1..],
    _ => item,
  };
  match item {
    [] => false,
    [_] => true,
    [expression, alias] => !is_reserved(alias) || is_operator(expression),
    [_, middle, alias] if *middle == "as" => !is_reserved(alias),
    words => words.iter().skip(1).step_by(2).all(|word| is_operator(word)),
  }
}

fn is_operator(word: &str) -> bool {
  !word.is_empty() && word.chars().all(|c| "+-*/%|=<>!".contains(c))
}

fn update_target_is_well_formed(words: &[String], table: usize) -> bool {
  if !words.get(table).is_some_and(|word| table_name(word).is_some()) {
    return false;
  }
  match (words.get(table + 1).map(String::as_str), words.get(table + 2).map(String::as_str)) {
    (Some("set"), _) => true,
    (Some("as"), _) => words.get(table + 3).is_some_and(|word| word == "set"),
    (Some(alias), Some("set")) => !is_reserved(alias),
    _ => false,
  }
}

fn insert_target_is_well_formed(words: &[String], table: usize) -> bool {
  words.get(table).is_some_and(|word| table_name(word).is_some())
    && words
      .get(table + 1)
      .is_some_and(|next| matches!(next.as_str(), "values" | "value" | "(" | "select" | "default" | "with" | "overriding"))
}

fn is_reserved(word: &str) -> bool {
  SQL_RESERVED.contains(&word) || ENGLISH_STOPWORDS.contains(&word)
}

fn sql_words(sql: &str) -> Vec<String> {
  let mut words = Vec::new();
  let mut current = String::new();
  let mut quote: Option<char> = None;
  for c in sql.chars() {
    if let Some(open) = quote {
      current.push(c);
      let closes = match open {
        '[' => c == ']',
        other => c == other,
      };
      if closes {
        quote = None;
      }
      continue;
    }
    match c {
      '"' | '`' | '[' => {
        current.push(c);
        quote = Some(c);
      }
      '\'' => {
        flush(&mut words, &mut current);
        quote = Some('\'');
        current.push(c);
      }
      '(' | ')' | ',' | ';' => {
        flush(&mut words, &mut current);
        words.push(c.to_string());
      }
      c if c.is_whitespace() => flush(&mut words, &mut current),
      c => current.push(c),
    }
  }
  flush(&mut words, &mut current);
  words
}

fn flush(words: &mut Vec<String>, current: &mut String) {
  if !current.is_empty() {
    words.push(std::mem::take(current));
  }
}

fn table_name(word: &str) -> Option<String> {
  if word.starts_with('\'') || word.starts_with('?') || word.starts_with('$') || word == "(" {
    return None;
  }
  let mut segments = Vec::new();
  for segment in word.split('.') {
    let quoted = segment.len() >= 2
      && matches!(
        (segment.chars().next(), segment.chars().last()),
        (Some('"'), Some('"')) | (Some('`'), Some('`')) | (Some('['), Some(']'))
      );
    let name = if quoted {
      segment[1..segment.len() - 1].to_owned()
    } else {
      segment.to_ascii_lowercase()
    };
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_' || (quoted && c != '"')) {
      return None;
    }
    segments.push(name);
  }
  if segments.iter().any(|segment| is_reserved(&segment.to_ascii_lowercase())) {
    return None;
  }
  Some(segments.join("."))
}

fn snake_case(name: &str) -> String {
  let mut snake = String::new();
  for (index, c) in name.chars().enumerate() {
    if c.is_uppercase() && index > 0 {
      snake.push('_');
    }
    snake.extend(c.to_lowercase());
  }
  snake
}

#[cfg(test)]
mod tests {
  use super::*;

  fn accesses(sql: &str) -> Vec<(String, bool)> {
    table_accesses(sql).into_iter().map(|access| (access.table, access.write)).collect()
  }

  #[test]
  fn should_extract_read_and_write_tables_from_sql() {
    assert_eq!(accesses("SELECT id, name FROM users WHERE id = $1"), [("users".to_owned(), false)]);
    assert_eq!(
      accesses("select * from orders o join \"Users\" u on u.id = o.user_id"),
      [("orders".to_owned(), false), ("Users".to_owned(), false)]
    );
    assert_eq!(accesses("INSERT INTO invoices (id) VALUES ($1)"), [("invoices".to_owned(), true)]);
    assert_eq!(accesses("UPDATE public.users SET name = $1"), [("public.users".to_owned(), true)]);
    assert_eq!(accesses("DELETE FROM sessions WHERE id = ?"), [("sessions".to_owned(), true)]);
    assert_eq!(accesses("SELECT EXTRACT(YEAR FROM created_at) FROM orders FOR UPDATE"), [("orders".to_owned(), false)]);
    assert_eq!(accesses("SELECT * FROM (SELECT 1) t"), []);
  }

  #[test]
  fn should_reject_english_sentences_and_half_sql() {
    for text in [
      "update the docs from now",
      "select from where",
      "select a few things from the list",
      "insert into the queue later",
      "delete the rows from users",
      "update users when ready",
      "select users from the table",
    ] {
      assert_eq!(accesses(text), [], "{text}");
    }
  }

  #[test]
  fn should_accept_well_formed_statements_after_leading_comments() {
    assert_eq!(accesses("-- fetch\n/* users */ SELECT count(*) AS n, max(id) FROM users"), [("users".to_owned(), false)]);
    assert_eq!(accesses("UPDATE users u SET name = $1"), [("users".to_owned(), true)]);
    assert_eq!(
      accesses("INSERT INTO audit SELECT * FROM events"),
      [("audit".to_owned(), true), ("events".to_owned(), false)]
    );
    assert_eq!(accesses("SELECT a + b FROM totals"), [("totals".to_owned(), false)]);
    assert_eq!(sql_text(&ArgSummary::Str("  /* x */ select 1".into())).as_deref(), Some("select 1"));
  }

  #[test]
  fn should_only_treat_statement_shaped_strings_as_sql() {
    assert!(sql_text(&ArgSummary::Str("select from users failed".into())).is_some());
    assert_eq!(sql_text(&ArgSummary::Str("failed to select from users".into())), None);
    assert_eq!(snake_case("OrderItem"), "order_item");
  }
}
