use atlas_core::{ArgSummary, Attrs, EdgeKind, EntityKey, FileFacts, LanguageFamily, LocalSymbolId, NodeRef, ReceiverHint, RefKind, ReferenceFact, Severity, Span};
use serde_json::json;

use super::detector;
use crate::cert::Cert;
use crate::lex::{Token, TokenKind};
use crate::sink::{Sink, attrs};
use crate::view::FileView;

pub struct ConfigEnvDetector;

detector!(ConfigEnvDetector, "config-env", 2);

const RUST_ENV_FUNCTIONS: [&[&str]; 4] = [&["std", "env", "var"], &["std", "env", "var_os"], &["dotenvy", "var"], &["dotenv", "var"]];
const CONFIG_GETTERS: [&str; 2] = ["get", "getOrThrow"];

impl ConfigEnvDetector {
  fn prefilter(&self, facts: &FileFacts) -> bool {
    match facts.file.language.family() {
      LanguageFamily::Ts => !facts.file.is_declaration_file,
      LanguageFamily::Rust => facts
        .references
        .iter()
        .any(|reference| reference.path.iter().any(|segment| matches!(segment.as_str(), "env" | "var" | "var_os" | "option_env"))),
      _ => false,
    }
  }

  fn run(&self, view: &FileView<'_>, sink: &mut Sink) {
    match view.family() {
      LanguageFamily::Rust => self.rust_env(view, sink),
      _ => {
        if view.source.contains("process") || view.source.contains("import.meta") {
          self.ts_env(view, sink);
        }
        self.ts_config(view, sink);
      }
    }
  }

  fn ts_env(&self, view: &FileView<'_>, sink: &mut Sink) {
    let tokens = view.tokens();
    for (index, token) in tokens.iter().enumerate() {
      let Some(after_env) = env_object_end(tokens, index, token) else { continue };
      let names = env_names(tokens, index, after_env);
      if names.is_empty() {
        if tokens.get(after_env).is_some_and(|next| next.is_punct("[")) {
          sink.diagnostic(
            Severity::Info,
            "env-dynamic-name",
            "process.env is indexed with a non-literal name".to_owned(),
            view.token_span(token),
          );
        }
        continue;
      }
      if is_env_write(tokens, index, &names) {
        continue;
      }
      let innermost = view.innermost_symbol(token.start);
      for (name, name_token, binding) in names {
        let Some(span) = view.token_span(name_token) else { continue };
        let from = binding.and_then(|binding| bound_symbol(view, &binding, token.start)).or(innermost);
        emit_env(view, sink, from, &name, span, Attrs::new());
      }
    }
  }

  fn ts_config(&self, view: &FileView<'_>, sink: &mut Sink) {
    for reference in view.facts.references.iter().filter(|reference| reference.kind == RefKind::MethodCall) {
      let Some((method, receiver)) = reference.path.split_last() else { continue };
      if !CONFIG_GETTERS.contains(&method.as_str()) {
        continue;
      }
      let Some(confidence) = config_receiver_confidence(view, reference, receiver) else {
        continue;
      };
      let Some(ArgSummary::Str(key)) = reference.args.first() else { continue };
      let config = EntityKey::ConfigKey {
        repo: view.repo().clone(),
        key: key.clone(),
      };
      let cert = Cert::heuristic(confidence, "config-service-get");
      sink.entity(config.clone(), reference.span, cert, Attrs::new());
      sink.edge(
        EdgeKind::ReadsConfig,
        view.node(reference.from),
        NodeRef::Key(config),
        reference.span,
        cert,
        attrs([("receiver", json!(receiver.join(".")))]),
      );
    }
  }

  fn rust_env(&self, view: &FileView<'_>, sink: &mut Sink) {
    for reference in &view.facts.references {
      let (is_env, compile_time) = match reference.kind {
        RefKind::Call => (is_rust_env_function(view, &reference.path), false),
        RefKind::MacroCall => (
          matches!(reference.path.as_slice(), [name] if name == "env" || name == "option_env") || reference.path == ["std", "env"],
          true,
        ),
        _ => (false, false),
      };
      if !is_env {
        continue;
      }
      let Some(ArgSummary::Str(name)) = reference.args.first() else {
        sink.diagnostic(
          Severity::Info,
          "env-dynamic-name",
          format!("{} is called with a non-literal variable name", reference.path.join("::")),
          Some(reference.span),
        );
        continue;
      };
      let extra = if compile_time { attrs([("compile_time", json!(true))]) } else { Attrs::new() };
      emit_env(view, sink, reference.from, name, reference.span, extra);
    }
  }
}

fn emit_env(view: &FileView<'_>, sink: &mut Sink, from: Option<LocalSymbolId>, name: &str, span: Span, mut extra: Attrs) {
  let env = EntityKey::EnvVar {
    repo: view.repo().clone(),
    name: name.to_owned(),
  };
  if view.is_test_context(from) {
    extra.insert("in_test".to_owned(), json!(true));
  }
  sink.entity(env.clone(), span, Cert::Syntactic, Attrs::new());
  sink.edge(EdgeKind::ReadsEnv, view.node(from), NodeRef::Key(env), span, Cert::Syntactic, extra);
}

fn is_rust_env_function(view: &FileView<'_>, path: &[String]) -> bool {
  RUST_ENV_FUNCTIONS.iter().any(|full| view.rust_path_is(path, full))
}

fn env_object_end(tokens: &[Token], index: usize, token: &Token) -> Option<usize> {
  let previous = index.checked_sub(1).and_then(|previous| tokens.get(previous));
  if previous.is_some_and(|previous| previous.is_punct(".") || previous.is_punct("?.")) {
    return None;
  }
  let at = |offset: usize| tokens.get(index + offset);
  if token.is_ident("process") && at(1).is_some_and(|dot| dot.is_punct(".") || dot.is_punct("?.")) && at(2).is_some_and(|env| env.is_ident("env")) {
    return Some(index + 3);
  }
  let is_import_meta = token.is_ident("import")
    && at(1).is_some_and(|dot| dot.is_punct("."))
    && at(2).is_some_and(|meta| meta.is_ident("meta"))
    && at(3).is_some_and(|dot| dot.is_punct("."))
    && at(4).is_some_and(|env| env.is_ident("env"));
  is_import_meta.then_some(index + 5)
}

fn is_env_write(tokens: &[Token], start: usize, names: &[(String, &Token, Option<String>)]) -> bool {
  if start
    .checked_sub(1)
    .and_then(|previous| tokens.get(previous))
    .is_some_and(|previous| previous.is_ident("delete"))
  {
    return true;
  }
  let [(_, name_token, None)] = names else { return false };
  let Some(name_index) = tokens.iter().position(|token| std::ptr::eq(token, *name_token)) else {
    return false;
  };
  let after_name = if tokens.get(name_index + 1).is_some_and(|close| close.is_punct("]")) {
    name_index + 2
  } else {
    name_index + 1
  };
  match (tokens.get(after_name), tokens.get(after_name + 1)) {
    (Some(operator), _) if matches!(operator.text.as_str(), "=" | "+=" | "-=") && operator.kind == TokenKind::Punct => true,
    (Some(logical), Some(equals)) => matches!(logical.text.as_str(), "??" | "||" | "&&") && equals.is_punct("=") && logical.end == equals.start,
    _ => false,
  }
}

fn bound_symbol(view: &FileView<'_>, binding: &str, byte: u32) -> Option<LocalSymbolId> {
  view
    .facts
    .symbols
    .iter()
    .find(|symbol| symbol.name == binding && symbol.span.contains_byte(byte))
    .map(|symbol| symbol.local)
}

fn env_names(tokens: &[Token], start: usize, after_env: usize) -> Vec<(String, &Token, Option<String>)> {
  let next = tokens.get(after_env);
  let name = tokens.get(after_env + 1);
  match (next, name) {
    (Some(dot), Some(name)) if (dot.is_punct(".") || dot.is_punct("?.")) && name.kind == TokenKind::Ident => return vec![(name.text.clone(), name, None)],
    (Some(open), Some(name)) if open.is_punct("[") && name.kind == TokenKind::Str && tokens.get(after_env + 2).is_some_and(|close| close.is_punct("]")) => {
      return vec![(name.text.clone(), name, None)];
    }
    _ => {}
  }
  destructured_names(tokens, start)
}

fn destructured_names(tokens: &[Token], start: usize) -> Vec<(String, &Token, Option<String>)> {
  let Some(equals) = start.checked_sub(1).and_then(|index| tokens.get(index)) else {
    return Vec::new();
  };
  let Some(close) = start.checked_sub(2) else { return Vec::new() };
  if !equals.is_punct("=") || !tokens.get(close).is_some_and(|token| token.is_punct("}")) {
    return Vec::new();
  }
  let mut depth = 0i32;
  let mut open = None;
  for index in (0..=close).rev() {
    let Some(token) = tokens.get(index) else { break };
    if token.is_punct("}") {
      depth += 1;
    } else if token.is_punct("{") {
      depth -= 1;
      if depth == 0 {
        open = Some(index);
        break;
      }
    }
  }
  let Some(open) = open else { return Vec::new() };
  let mut names = Vec::new();
  let mut depth = 0i32;
  for index in open + 1..close {
    let Some(token) = tokens.get(index) else { break };
    match token.text.as_str() {
      "{" | "[" | "(" if token.kind == TokenKind::Punct => depth += 1,
      "}" | "]" | ")" if token.kind == TokenKind::Punct => depth -= 1,
      _ if depth == 0 && token.kind == TokenKind::Ident => {
        let previous = tokens.get(index - 1);
        let is_key = previous.is_some_and(|previous| previous.is_punct("{") || previous.is_punct(",") || previous.is_punct("..."));
        if is_key && !previous.is_some_and(|previous| previous.is_punct("...")) {
          let alias = match (tokens.get(index + 1), tokens.get(index + 2)) {
            (Some(colon), Some(alias)) if colon.is_punct(":") && alias.kind == TokenKind::Ident => alias.text.clone(),
            _ => token.text.clone(),
          };
          names.push((token.text.clone(), token, Some(alias)));
        }
      }
      _ => {}
    }
  }
  names
}

fn config_receiver_confidence(view: &FileView<'_>, reference: &ReferenceFact, receiver: &[String]) -> Option<f32> {
  let typed_config = |type_text: &str| type_text.trim() == "ConfigService" && view.imports_named("ConfigService", &["@nestjs/config"], "ConfigService");
  match receiver {
    [this, field] if this == "this" => {
      let class = reference.from.and_then(|local| view.class_of(local))?;
      if view.field_type(class, field).is_some_and(|type_text| typed_config(&type_text)) {
        return Some(0.8);
      }
      field.to_ascii_lowercase().contains("config").then_some(0.6)
    }
    [name] => {
      if let Some(ReceiverHint::TypeHint(path)) = &reference.receiver
        && path.last().is_some_and(|last| typed_config(last))
      {
        return Some(0.8);
      }
      name.to_ascii_lowercase().contains("config").then_some(0.6)
    }
    _ => None,
  }
}

#[cfg(test)]
mod tests {
  use atlas_core::LanguageFamily;

  use super::*;
  use crate::lex::lex;

  fn names(source: &str) -> Vec<String> {
    let tokens = lex(source, LanguageFamily::Ts);
    tokens
      .iter()
      .enumerate()
      .filter_map(|(index, token)| env_object_end(&tokens, index, token).map(|after| (index, after)))
      .flat_map(|(index, after)| env_names(&tokens, index, after).into_iter().map(|(name, _, _)| name).collect::<Vec<_>>())
      .collect()
  }

  #[test]
  fn should_read_env_names_from_member_index_and_destructuring_forms() {
    assert_eq!(names("const a = process.env.A ?? process.env['B'];"), ["A", "B"]);
    assert_eq!(names("const { C, D: renamed, E = 'x', ...rest } = process.env;"), ["C", "D", "E"]);
    assert_eq!(names("const url = import.meta.env.VITE_API_URL;"), ["VITE_API_URL"]);
    assert_eq!(names("const v = process.env[key]; const w = other.process.env.X;"), Vec::<String>::new());
  }
}
