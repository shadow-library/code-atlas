use atlas_core::{ArgSummary, ExportFact, ImportBinding, ImportFact, LocalSymbolId, ReExportBinding};
use tree_sitter::Node;

use crate::args::summarize;
use crate::syntax::{children, has_token, span_of, string_value, text, unwrap_expression};

pub(crate) fn import_statement(node: Node<'_>, source: &str) -> Option<ImportFact> {
  let statement_type_only = has_token(node, "type");
  if let Some(clause) = children(node).into_iter().find(|child| child.kind() == "import_require_clause") {
    let local = children(clause).into_iter().find(|child| child.kind() == "identifier")?;
    let specifier = clause.child_by_field_name("source")?;
    return Some(ImportFact::TsRequire {
      specifier: ArgSummary::Str(string_value(specifier, source)),
      bindings: vec![ImportBinding::Namespace {
        local: text(local, source).to_owned(),
      }],
      scope: None,
      span: span_of(node),
    });
  }
  let specifier = string_value(node.child_by_field_name("source")?, source);
  let mut bindings = Vec::new();
  if let Some(clause) = children(node).into_iter().find(|child| child.kind() == "import_clause") {
    for part in children(clause) {
      match part.kind() {
        "identifier" => bindings.push(ImportBinding::Default {
          local: text(part, source).to_owned(),
        }),
        "namespace_import" => {
          if let Some(local) = children(part).into_iter().find(|child| child.kind() == "identifier") {
            bindings.push(ImportBinding::Namespace {
              local: text(local, source).to_owned(),
            });
          }
        }
        "named_imports" => bindings.extend(children(part).into_iter().filter_map(|specifier| named_import(specifier, source, statement_type_only))),
        _ => {}
      }
    }
  }
  Some(ImportFact::TsImport {
    specifier,
    bindings,
    type_only: statement_type_only,
    span: span_of(node),
  })
}

fn named_import(specifier: Node<'_>, source: &str, statement_type_only: bool) -> Option<ImportBinding> {
  if specifier.kind() != "import_specifier" {
    return None;
  }
  let imported = module_export_name(specifier.child_by_field_name("name")?, source);
  let local = specifier
    .child_by_field_name("alias")
    .map_or_else(|| imported.clone(), |alias| module_export_name(alias, source));
  Some(ImportBinding::Named {
    imported,
    local,
    type_only: statement_type_only || has_token(specifier, "type"),
  })
}

fn module_export_name(node: Node<'_>, source: &str) -> String {
  match node.kind() {
    "string" => string_value(node, source),
    _ => text(node, source).to_owned(),
  }
}

pub(crate) fn require_call<'t>(value: Node<'t>, source: &str) -> Option<Node<'t>> {
  let value = unwrap_expression(value);
  if value.kind() != "call_expression" {
    return None;
  }
  let callee = value.child_by_field_name("function")?;
  (callee.kind() == "identifier" && text(callee, source) == "require").then_some(value)
}

pub(crate) fn module_specifier(call: Node<'_>, source: &str) -> ArgSummary {
  let argument = call.child_by_field_name("arguments").and_then(|arguments| children(arguments).into_iter().next());
  argument.map_or(ArgSummary::Truncated, |argument| summarize(argument, source, 0))
}

pub(crate) fn require_bindings(pattern: Node<'_>, source: &str) -> Vec<ImportBinding> {
  match pattern.kind() {
    "identifier" => vec![ImportBinding::Namespace {
      local: text(pattern, source).to_owned(),
    }],
    "object_pattern" => children(pattern).into_iter().filter_map(|property| destructured_binding(property, source)).collect(),
    _ => Vec::new(),
  }
}

fn destructured_binding(property: Node<'_>, source: &str) -> Option<ImportBinding> {
  let (imported, local) = match property.kind() {
    "shorthand_property_identifier_pattern" => (text(property, source).to_owned(), text(property, source).to_owned()),
    "object_assignment_pattern" => {
      let left = property.child_by_field_name("left")?;
      (text(left, source).to_owned(), text(left, source).to_owned())
    }
    "pair_pattern" => {
      let key = property.child_by_field_name("key")?;
      let value = property.child_by_field_name("value")?;
      let value = match value.kind() {
        "assignment_pattern" => value.child_by_field_name("left")?,
        _ => value,
      };
      if value.kind() != "identifier" {
        return None;
      }
      (module_export_name(key, source), text(value, source).to_owned())
    }
    _ => return None,
  };
  Some(ImportBinding::Named {
    imported,
    local,
    type_only: false,
  })
}

pub(crate) fn require_fact(call: Node<'_>, bindings: Vec<ImportBinding>, scope: Option<LocalSymbolId>, span_node: Node<'_>, source: &str) -> ImportFact {
  ImportFact::TsRequire {
    specifier: module_specifier(call, source),
    bindings,
    scope,
    span: span_of(span_node),
  }
}

pub(crate) fn export_list(node: Node<'_>, source: &str) -> Vec<ExportFact> {
  let statement_type_only = has_token(node, "type");
  let span = span_of(node);
  let specifier = node.child_by_field_name("source").map(|source_node| string_value(source_node, source));
  if let Some(namespace) = children(node).into_iter().find(|child| child.kind() == "namespace_export") {
    let name = children(namespace).into_iter().next().map(|name| module_export_name(name, source));
    return specifier
      .map(|specifier| ExportFact::ReExportAll {
        specifier,
        namespace: name,
        type_only: statement_type_only,
        span,
      })
      .into_iter()
      .collect();
  }
  let Some(clause) = children(node).into_iter().find(|child| child.kind() == "export_clause") else {
    return specifier
      .filter(|_| has_token(node, "*"))
      .map(|specifier| ExportFact::ReExportAll {
        specifier,
        namespace: None,
        type_only: statement_type_only,
        span,
      })
      .into_iter()
      .collect();
  };
  let pairs: Vec<(String, String, bool)> = children(clause)
    .into_iter()
    .filter(|child| child.kind() == "export_specifier")
    .filter_map(|export| {
      let name = module_export_name(export.child_by_field_name("name")?, source);
      let alias = export.child_by_field_name("alias").map_or_else(|| name.clone(), |alias| module_export_name(alias, source));
      Some((name, alias, statement_type_only || has_token(export, "type")))
    })
    .collect();
  match specifier {
    Some(specifier) => vec![ExportFact::ReExport {
      specifier,
      bindings: pairs
        .into_iter()
        .map(|(imported, exported, type_only)| ReExportBinding { imported, exported, type_only })
        .collect(),
      type_only: statement_type_only,
      span,
    }],
    None => pairs
      .into_iter()
      .map(|(local, exported, type_only)| ExportFact::Named { local, exported, type_only, span })
      .collect(),
  }
}
