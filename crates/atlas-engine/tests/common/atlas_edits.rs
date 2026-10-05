use std::ffi::OsString;
use std::fs;
use std::ops::Range;
use std::path::PathBuf;

use atlas_core::{ContentHash, EntityKind, Language, RelPath, RepoName};
use atlas_edit::{
  ApplyOptions, EditPlan, EditResult, EditScope, FileTarget, Nesting, Pattern, ReplaceContent, ReplaceInFiles, SymbolOp, SymbolTarget, Syntax, plan_symbol_edit, replace_content,
  replace_in_files,
};
use atlas_engine::Engine;
use atlas_store::EntityFilter;

use super::SplitMix;

struct Declaration {
  file: FileTarget,
  name: String,
  kind: EntityKind,
  span: Range<usize>,
  floor: usize,
  syntax: Syntax,
}

pub struct AtlasEdits {
  scope: EditScope,
  options: ApplyOptions,
}

const ATTEMPTS: usize = 20;

fn is_plain_identifier(name: &str) -> bool {
  name.len() >= 3 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.starts_with(|c: char| c.is_ascii_digit())
}

impl AtlasEdits {
  pub fn new(engine: &Engine) -> Self {
    let mut lock: OsString = engine.db_path().into_os_string();
    lock.push(".edit.lock");
    Self {
      scope: EditScope::new(&engine.project()).unwrap(),
      options: ApplyOptions::new(PathBuf::from(lock)),
    }
  }

  pub fn apply(&self, plan: &EditPlan) -> EditResult<()> {
    atlas_edit::apply(plan, &self.scope, &self.options).map(|_| ())
  }

  pub fn touched(plan: &EditPlan) -> Vec<(RepoName, RelPath)> {
    plan.files.iter().map(|file| (file.file.repo.clone(), file.file.path.clone())).collect()
  }

  pub fn plan(&self, engine: &Engine, rng: &mut SplitMix, step: usize) -> (String, EditPlan) {
    let declarations = top_level_declarations(engine);
    for _ in 0..ATTEMPTS {
      let declaration = rng.pick(&declarations).unwrap();
      let planned = match rng.below(5) {
        0 => self.rename_in_file(declaration, step),
        1 => self.rename_everywhere(declaration, step),
        2 => self.insert_caller_after(declaration, step),
        3 => self.delete(declaration),
        _ => self.replace_with_renamed_copy(declaration, step),
      };
      if let Some(planned) = planned {
        return planned;
      }
    }
    panic!("step {step}: no atlas edit could be planned in {ATTEMPTS} attempts");
  }

  fn renamed(declaration: &Declaration, step: usize) -> String {
    match declaration.syntax {
      Syntax::Rust => format!("{}_edit{step}", declaration.name),
      Syntax::TypeScript => format!("{}Edit{step}", declaration.name),
    }
  }

  fn rename_in_file(&self, declaration: &Declaration, step: usize) -> Option<(String, EditPlan)> {
    let new_name = Self::renamed(declaration, step);
    let request = ReplaceContent {
      pattern: Pattern::Regex(format!(r"\b{}\b", declaration.name)),
      text: new_name.clone(),
      all: true,
    };
    let planned = replace_content(&self.scope, &declaration.file, &request).ok()?;
    Some((format!("replace_content {} {} -> {new_name}", declaration.file, declaration.name), planned.plan))
  }

  fn rename_everywhere(&self, declaration: &Declaration, step: usize) -> Option<(String, EditPlan)> {
    let new_name = Self::renamed(declaration, step);
    let mut request = ReplaceInFiles {
      pattern: Pattern::Regex(format!(r"\b{}\b", declaration.name)),
      text: new_name.clone(),
      include: Vec::new(),
      exclude: Vec::new(),
      occurrences: None,
      expected_count: None,
      dry_run: true,
    };
    let counted = replace_in_files(&self.scope, &request).ok()?;
    request.dry_run = false;
    request.expected_count = Some(counted.total_matches);
    let planned = replace_in_files(&self.scope, &request).ok()?;
    let files = planned.text.plan.files.len();
    let description = format!("replace_in_files {} -> {new_name} ({} matches in {files} files)", declaration.name, counted.total_matches);
    Some((description, planned.text.plan))
  }

  fn symbol_edit(&self, declaration: &Declaration, op: &SymbolOp) -> Option<EditPlan> {
    let location = self.scope.locate(&declaration.file).ok()?;
    let target = SymbolTarget {
      file: declaration.file.clone(),
      expected: ContentHash::of(&fs::read(location.absolute).ok()?),
      span: declaration.span.clone(),
      floor: declaration.floor,
      syntax: declaration.syntax,
      nesting: Nesting::TopLevel,
    };
    plan_symbol_edit(&self.scope, &target, op).ok().map(|planned| planned.plan)
  }

  fn insert_caller_after(&self, declaration: &Declaration, step: usize) -> Option<(String, EditPlan)> {
    let text = match declaration.syntax {
      Syntax::Rust => format!("pub fn atlas_probe_{step}() {{\n  let _ = {}();\n}}\n", declaration.name),
      Syntax::TypeScript => format!("export function atlasProbe{step}(): unknown {{\n  return {}();\n}}\n", declaration.name),
    };
    let plan = self.symbol_edit(declaration, &SymbolOp::InsertAfter(text))?;
    Some((format!("insert_after {} {}", declaration.file, declaration.name), plan))
  }

  fn delete(&self, declaration: &Declaration) -> Option<(String, EditPlan)> {
    let plan = self.symbol_edit(declaration, &SymbolOp::SafeDelete)?;
    Some((format!("delete {} {} ({})", declaration.file, declaration.name, declaration.kind.as_str()), plan))
  }

  fn replace_with_renamed_copy(&self, declaration: &Declaration, step: usize) -> Option<(String, EditPlan)> {
    let location = self.scope.locate(&declaration.file).ok()?;
    let content = fs::read_to_string(location.absolute).ok()?;
    let original = content.get(declaration.span.clone())?;
    let new_name = Self::renamed(declaration, step);
    let doc = match declaration.syntax {
      Syntax::Rust => format!("/// Replaced at step {step}.\n"),
      Syntax::TypeScript => format!("/** Replaced at step {step}. */\n"),
    };
    let text = format!("{doc}{}", original.replacen(&declaration.name, &new_name, 1));
    let plan = self.symbol_edit(declaration, &SymbolOp::ReplaceBody(text))?;
    Some((format!("replace_body {} {} -> {new_name}", declaration.file, declaration.name), plan))
  }
}

fn top_level_declarations(engine: &Engine) -> Vec<Declaration> {
  let project = engine.project();
  let mut declarations = Vec::new();
  for repo in &project.repos {
    let files = engine.store().read(|tx| tx.files_in_repo(&repo.name)).unwrap();
    for file in files {
      let syntax = match file.language {
        Some(Language::Rust) => Syntax::Rust,
        Some(Language::TypeScript | Language::Tsx) => Syntax::TypeScript,
        _ => continue,
      };
      let symbols: Vec<(String, EntityKind, Range<usize>)> = engine
        .store()
        .read(|tx| tx.entities_in_file(&file.repo, &file.path, &EntityFilter::default()))
        .unwrap()
        .into_iter()
        .filter(|entity| entity.kind.is_code_symbol() && entity.path.as_ref() == Some(&file.path))
        .filter_map(|entity| entity.span.map(|span| (entity.name, entity.kind, span.start_byte as usize..span.end_byte as usize)))
        .collect();
      let top_level: Vec<&(String, EntityKind, Range<usize>)> = symbols
        .iter()
        .filter(|(_, _, span)| !symbols.iter().any(|(_, _, other)| other != span && other.start <= span.start && span.end <= other.end))
        .collect();
      for (name, kind, span) in &top_level {
        if !matches!(kind, EntityKind::Function | EntityKind::Class | EntityKind::Struct) || !is_plain_identifier(name) {
          continue;
        }
        let floor = top_level.iter().map(|(_, _, other)| other.end).filter(|end| *end <= span.start).max().unwrap_or(0);
        declarations.push(Declaration {
          file: FileTarget::new(file.repo.clone(), file.path.clone()),
          name: name.clone(),
          kind: kind.clone(),
          span: span.clone(),
          floor,
          syntax,
        });
      }
    }
  }
  declarations.sort_by_key(|declaration| (declaration.file.to_string(), declaration.span.start));
  declarations
}
