use atlas_core::{ArgSummary, LocalSymbolId, TemplatePart};

use crate::cert::Cert;
use crate::view::FileView;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Piece {
  Lit(String),
  Dyn(ArgSummary),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StringEval {
  pub pieces: Vec<Piece>,
  pub cert: Cert,
}

impl StringEval {
  pub fn static_value(&self) -> Option<String> {
    self
      .pieces
      .iter()
      .map(|piece| match piece {
        Piece::Lit(value) => Some(value.as_str()),
        Piece::Dyn(_) => None,
      })
      .collect()
  }

  pub fn pattern(&self, placeholder: &str) -> String {
    self
      .pieces
      .iter()
      .map(|piece| match piece {
        Piece::Lit(value) => value.as_str(),
        Piece::Dyn(_) => placeholder,
      })
      .collect()
  }

  pub fn leading_dynamic(&self) -> Option<&ArgSummary> {
    match self.pieces.first() {
      Some(Piece::Dyn(arg)) => Some(arg),
      _ => None,
    }
  }

  pub fn has_literal_text(&self) -> bool {
    self.pieces.iter().any(|piece| matches!(piece, Piece::Lit(value) if value.chars().any(|c| c != '/')))
  }
}

pub(crate) fn evaluate(view: &FileView<'_>, scope: Option<LocalSymbolId>, arg: &ArgSummary) -> Option<StringEval> {
  let mut eval = StringEval {
    pieces: Vec::new(),
    cert: Cert::Syntactic,
  };
  if !collect(view, scope, arg, &mut eval) {
    return None;
  }
  Some(eval)
}

fn collect(view: &FileView<'_>, scope: Option<LocalSymbolId>, arg: &ArgSummary, eval: &mut StringEval) -> bool {
  match arg {
    ArgSummary::Str(value) => push_literal(eval, value),
    ArgSummary::Template(parts) => {
      for part in parts {
        match part {
          TemplatePart::Lit(value) => push_literal(eval, value),
          TemplatePart::Expr(inner) => {
            if !collect(view, scope, inner, eval) {
              eval.pieces.push(Piece::Dyn(inner.clone()));
            }
          }
        }
      }
    }
    ArgSummary::Concat(items) => {
      let mut inner = StringEval {
        pieces: Vec::new(),
        cert: Cert::Resolved,
      };
      let mut saw_string = false;
      for item in items {
        if collect(view, scope, item, &mut inner) {
          saw_string = true;
        } else {
          inner.pieces.push(Piece::Dyn(item.clone()));
        }
      }
      if !saw_string {
        return false;
      }
      eval.cert = eval.cert.weaker(inner.cert);
      for piece in inner.pieces {
        match piece {
          Piece::Lit(value) => push_literal(eval, &value),
          dynamic => eval.pieces.push(dynamic),
        }
      }
    }
    ArgSummary::Path(path) => {
      let [name] = path.as_slice() else { return false };
      if view.enclosing_scope_contains_binding(scope, name) {
        return false;
      }
      let Some(value) = view.module_const_string(name) else { return false };
      eval.cert = eval.cert.weaker(Cert::Resolved);
      push_literal(eval, &value);
    }
    _ => return false,
  }
  true
}

fn push_literal(eval: &mut StringEval, value: &str) {
  if let Some(Piece::Lit(previous)) = eval.pieces.last_mut() {
    previous.push_str(value);
    return;
  }
  eval.pieces.push(Piece::Lit(value.to_owned()));
}
