use std::fs;
use std::path::{Path, PathBuf};

use super::{SplitMix, walk_files};

pub struct Tree {
  pub root: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
  Body,
  Rename,
  Move,
  Delete,
  Add,
  Import,
  Route,
  Guard,
  Manifest,
  TestProbe,
  ProbeSource,
}

pub const ALL_EDITS: [EditKind; 11] = [
  EditKind::Body,
  EditKind::Rename,
  EditKind::Move,
  EditKind::Delete,
  EditKind::Add,
  EditKind::Import,
  EditKind::Route,
  EditKind::Guard,
  EditKind::Manifest,
  EditKind::TestProbe,
  EditKind::ProbeSource,
];

fn is_ident(c: char) -> bool {
  c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

fn is_ts(path: &Path) -> bool {
  matches!(path.extension().and_then(|ext| ext.to_str()), Some("ts" | "tsx"))
}

fn is_rs(path: &Path) -> bool {
  path.extension().and_then(|ext| ext.to_str()) == Some("rs")
}

pub fn replace_word(text: &str, from: &str, to: &str) -> String {
  let mut out = String::with_capacity(text.len());
  let mut rest = text;
  while let Some(index) = rest.find(from) {
    let before = rest[..index].chars().next_back();
    let after = rest[index + from.len()..].chars().next();
    out.push_str(&rest[..index]);
    match before.is_some_and(is_ident) || after.is_some_and(is_ident) {
      true => out.push_str(from),
      false => out.push_str(to),
    }
    rest = &rest[index + from.len()..];
  }
  out.push_str(rest);
  out
}

fn declared_names(text: &str, prefixes: &[&str]) -> Vec<String> {
  let mut names = Vec::new();
  for line in text.lines() {
    let trimmed = line.trim_start();
    for prefix in prefixes {
      if let Some(rest) = trimmed.strip_prefix(prefix) {
        let name: String = rest.chars().take_while(|c| is_ident(*c)).collect();
        if !name.is_empty() {
          names.push(name);
        }
      }
    }
  }
  names
}

const TS_EXPORTS: [&str; 5] = ["export function ", "export async function ", "export class ", "export const ", "export interface "];

impl Tree {
  fn repos(&self) -> Vec<PathBuf> {
    let mut repos: Vec<PathBuf> = fs::read_dir(&self.root).unwrap().flatten().map(|entry| entry.path()).filter(|path| path.is_dir()).collect();
    repos.sort();
    repos
  }

  fn code_files(&self) -> Vec<PathBuf> {
    walk_files(&self.root).into_iter().filter(|path| is_ts(path) || is_rs(path)).collect()
  }

  fn ts_files(&self) -> Vec<PathBuf> {
    self.code_files().into_iter().filter(|path| is_ts(path)).collect()
  }

  fn repo_of(&self, file: &Path) -> PathBuf {
    let relative = file.strip_prefix(&self.root).unwrap();
    self.root.join(relative.components().next().unwrap())
  }

  fn relative(&self, file: &Path) -> String {
    file.strip_prefix(&self.root).unwrap().to_string_lossy().into_owned()
  }

  pub fn apply(&self, rng: &mut SplitMix, step: usize) -> String {
    for _ in 0..64 {
      let kind = *rng.pick(&ALL_EDITS).unwrap();
      if let Some(description) = self.try_apply(kind, rng, step) {
        return format!("{kind:?}: {description}");
      }
    }
    panic!("no edit applied at step {step}");
  }

  pub fn try_apply(&self, kind: EditKind, rng: &mut SplitMix, step: usize) -> Option<String> {
    match kind {
      EditKind::Body => self.edit_body(rng, step),
      EditKind::Rename => self.rename_function(rng, step),
      EditKind::Move => self.move_file(rng, step),
      EditKind::Delete => self.delete_file(rng),
      EditKind::Add => self.add_file(rng, step),
      EditKind::Import => self.toggle_import(rng),
      EditKind::Route => self.change_route(rng, step),
      EditKind::Guard => self.change_guard(rng),
      EditKind::Manifest => self.change_manifest(rng),
      EditKind::TestProbe => self.add_test_probe(rng, step),
      EditKind::ProbeSource => self.toggle_probe_source(rng, step),
    }
  }

  fn edit_body(&self, rng: &mut SplitMix, step: usize) -> Option<String> {
    let file = rng.pick(&self.code_files())?.clone();
    let text = fs::read_to_string(&file).ok()?;
    let lines: Vec<&str> = text.lines().collect();
    let candidates: Vec<usize> = lines
      .iter()
      .enumerate()
      .filter(|(_, line)| {
        let trimmed = line.trim_start();
        let indented = line.len() > trimmed.len();
        match is_rs(&file) {
          true => indented && trimmed.starts_with("let "),
          false => indented && (trimmed.starts_with("return ") || trimmed.starts_with("const ")),
        }
      })
      .map(|(index, _)| index)
      .collect();
    let at = *rng.pick(&candidates)?;
    let indent: String = lines[at].chars().take_while(|c| c.is_whitespace()).collect();
    let statement = match is_rs(&file) {
      true => format!("{indent}let _touched{step} = {step};"),
      false => format!("{indent}const touched{step} = {step};"),
    };
    let mut edited: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
    edited.insert(at, statement);
    fs::write(&file, edited.join("\n") + "\n").ok()?;
    Some(format!("{} line {}", self.relative(&file), at + 1))
  }

  fn rename_function(&self, rng: &mut SplitMix, step: usize) -> Option<String> {
    let files = self.code_files();
    let file = rng.pick(&files)?.clone();
    let text = fs::read_to_string(&file).ok()?;
    let names = match is_rs(&file) {
      true => declared_names(&text, &["pub fn ", "pub async fn ", "fn "]),
      false => declared_names(&text, &["export function ", "export async function ", "function "]),
    };
    let name = rng.pick(&names)?.clone();
    if name == "main" || name == "router" {
      return None;
    }
    let renamed = format!("{name}R{step}");
    for path in &files {
      let Ok(content) = fs::read_to_string(path) else { continue };
      let replaced = replace_word(&content, &name, &renamed);
      if replaced != content {
        fs::write(path, replaced).ok()?;
      }
    }
    Some(format!("{name} -> {renamed} (declared in {})", self.relative(&file)))
  }

  fn move_file(&self, rng: &mut SplitMix, step: usize) -> Option<String> {
    let files = self.code_files();
    let file = rng.pick(&files)?.clone();
    let stem = file.file_stem()?.to_str()?.to_owned();
    let extension = file.extension()?.to_str()?.to_owned();
    if matches!(stem.as_str(), "index" | "lib" | "main" | "mod") || stem.contains('.') {
      return None;
    }
    let target = file.with_file_name(format!("{stem}_m{step}.{extension}"));
    let repo = self.repo_of(&file);
    if is_rs(&file) {
      let declaration = format!("pub mod {stem};");
      let root = ["src/lib.rs", "src/main.rs"]
        .iter()
        .map(|name| repo.join(name))
        .find(|root| fs::read_to_string(root).is_ok_and(|text| text.lines().any(|line| line.trim() == declaration)) && file.parent() == root.parent())?;
      let text = fs::read_to_string(&root).ok()?;
      let replaced = text.replacen(&declaration, &format!("#[path = \"{stem}_m{step}.rs\"]\n{declaration}"), 1);
      fs::rename(&file, &target).ok()?;
      fs::write(&root, replaced).ok()?;
      return Some(format!("{} -> {} with #[path]", self.relative(&file), self.relative(&target)));
    }
    fs::rename(&file, &target).ok()?;
    for path in self.code_files().into_iter().filter(|path| path.starts_with(&repo) && is_ts(path)) {
      let Ok(content) = fs::read_to_string(&path) else { continue };
      let replaced = content
        .replace(&format!("/{stem}'"), &format!("/{stem}_m{step}'"))
        .replace(&format!("/{stem}\""), &format!("/{stem}_m{step}\""));
      if replaced != content {
        fs::write(&path, replaced).ok()?;
      }
    }
    Some(format!("{} -> {}", self.relative(&file), self.relative(&target)))
  }

  fn delete_file(&self, rng: &mut SplitMix) -> Option<String> {
    let file = rng.pick(&self.code_files())?.clone();
    if matches!(file.file_name()?.to_str()?, "lib.rs" | "main.rs") {
      return None;
    }
    fs::remove_file(&file).ok()?;
    Some(self.relative(&file))
  }

  fn add_file(&self, rng: &mut SplitMix, step: usize) -> Option<String> {
    if rng.below(4) == 0 {
      let lib = self.repos().into_iter().map(|repo| repo.join("src/lib.rs")).find(|lib| lib.exists())?;
      let module = lib.with_file_name(format!("added{step}.rs"));
      fs::write(
        &module,
        format!("use crate::money::*;\n\npub fn added_{step}(value: i64) -> i64 {{\n  value + {step}\n}}\n"),
      )
      .ok()?;
      let mut text = fs::read_to_string(&lib).ok()?;
      text.push_str(&format!("pub mod added{step};\n"));
      fs::write(&lib, text).ok()?;
      return Some(self.relative(&module));
    }
    let files = self.ts_files();
    let sibling = rng.pick(&files)?.clone();
    let stem = sibling.file_stem()?.to_str()?.to_owned();
    let text = fs::read_to_string(&sibling).ok()?;
    let name = rng.pick(&declared_names(&text, &TS_EXPORTS))?.clone();
    let alias = rng.below(3) == 0 && self.repo_of(&sibling).ends_with("acme-api");
    let import = match alias {
      true => "import { formatMoney } from '@lib/money';\n".to_owned(),
      false => String::new(),
    };
    let target = sibling.with_file_name(format!("added{step}.ts"));
    let body = match alias {
      true => format!("{name}, formatMoney"),
      false => name.clone(),
    };
    fs::write(
      &target,
      format!("{import}import {{ {name} }} from './{stem}';\n\nexport function added{step}() {{\n  return [{body}];\n}}\n"),
    )
    .ok()?;
    Some(format!("{} importing {name}", self.relative(&target)))
  }

  fn toggle_import(&self, rng: &mut SplitMix) -> Option<String> {
    let files = self.code_files();
    let file = rng.pick(&files)?.clone();
    let text = fs::read_to_string(&file).ok()?;
    let lines: Vec<&str> = text.lines().collect();
    if rng.below(2) == 0 {
      let imports: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| (line.starts_with("import ") || line.starts_with("use ")) && line.ends_with(';'))
        .map(|(index, _)| index)
        .collect();
      let at = *rng.pick(&imports)?;
      let removed = lines[at].to_owned();
      let kept: Vec<&str> = lines.iter().enumerate().filter(|(index, _)| *index != at).map(|(_, line)| *line).collect();
      fs::write(&file, kept.join("\n") + "\n").ok()?;
      return Some(format!("removed `{removed}` from {}", self.relative(&file)));
    }
    if !is_ts(&file) {
      return None;
    }
    let siblings: Vec<PathBuf> = fs::read_dir(file.parent()?)
      .ok()?
      .flatten()
      .map(|entry| entry.path())
      .filter(|path| is_ts(path) && *path != file)
      .collect();
    let sibling = rng.pick(&siblings)?.clone();
    let name = rng.pick(&declared_names(&fs::read_to_string(&sibling).ok()?, &TS_EXPORTS))?.clone();
    let stem = sibling.file_stem()?.to_str()?.to_owned();
    let line = format!("import {{ {name} }} from './{stem}';");
    fs::write(&file, format!("{line}\n{text}")).ok()?;
    Some(format!("added `{line}` to {}", self.relative(&file)))
  }

  fn change_route(&self, rng: &mut SplitMix, step: usize) -> Option<String> {
    let markers = ["@Get('", "@Post('", "@Patch('", "@Delete('", "@Put('", "@Controller('", ".route(\"/", ".get('/", ".post('/"];
    let candidates: Vec<(PathBuf, &str)> = self
      .code_files()
      .into_iter()
      .filter_map(|path| {
        let text = fs::read_to_string(&path).ok()?;
        let marker = markers.iter().find(|marker| text.contains(**marker))?;
        Some((path, *marker))
      })
      .collect();
    let (file, marker) = rng.pick(&candidates)?.clone();
    let text = fs::read_to_string(&file).ok()?;
    let occurrences: Vec<usize> = text.match_indices(marker).map(|(index, _)| index).collect();
    let at = *rng.pick(&occurrences)? + marker.len();
    let mut edited = text.clone();
    edited.insert_str(at, &format!("v{step}/"));
    fs::write(&file, edited).ok()?;
    Some(format!("{} at byte {at}", self.relative(&file)))
  }

  fn change_guard(&self, rng: &mut SplitMix) -> Option<String> {
    let files = self.ts_files();
    let guards: Vec<(String, PathBuf)> = files
      .iter()
      .filter(|path| path.to_string_lossy().ends_with(".guard.ts"))
      .flat_map(|path| {
        let text = fs::read_to_string(path).unwrap_or_default();
        declared_names(&text, &["export class "]).into_iter().map(move |name| (name, path.clone()))
      })
      .collect();
    let users: Vec<PathBuf> = files
      .iter()
      .filter(|path| fs::read_to_string(path).is_ok_and(|text| text.contains("@UseGuards(")))
      .cloned()
      .collect();
    let file = rng.pick(&users)?.clone();
    let (guard, guard_path) = rng.pick(&guards)?.clone();
    let text = fs::read_to_string(&file).ok()?;
    let start = text.find("@UseGuards(")? + "@UseGuards(".len();
    let current: String = text[start..].chars().take_while(|c| is_ident(*c)).collect();
    if current.is_empty() || current == guard {
      return None;
    }
    let mut edited = format!("{}{}{}", &text[..start], guard, &text[start + current.len()..]);
    let specifier = relative_specifier(file.parent()?, &guard_path)?;
    edited = format!("import {{ {guard} }} from '{specifier}';\n{edited}");
    fs::write(&file, edited).ok()?;
    Some(format!("{current} -> {guard} in {}", self.relative(&file)))
  }

  fn add_test_probe(&self, rng: &mut SplitMix, step: usize) -> Option<String> {
    let dir = rng.pick(&self.ts_files())?.parent()?.to_path_buf();
    let spec = dir.join(format!("probe{step}.spec.ts"));
    fs::write(
      &spec,
      format!("describe('probe{step}', () => {{\n  it('works', () => {{\n    expect({step}).toBe({step});\n  }});\n}});\n"),
    )
    .ok()?;
    Some(self.relative(&spec))
  }

  fn toggle_probe_source(&self, rng: &mut SplitMix, step: usize) -> Option<String> {
    let specs: Vec<PathBuf> = self
      .ts_files()
      .into_iter()
      .filter(|path| {
        path
          .file_name()
          .and_then(|name| name.to_str())
          .is_some_and(|name| name.starts_with("probe") && name.ends_with(".spec.ts"))
      })
      .collect();
    let spec = rng.pick(&specs)?;
    let name = spec.file_name()?.to_str()?.trim_end_matches(".spec.ts").to_owned();
    let source = spec.with_file_name(format!("{name}.ts"));
    match source.exists() {
      true => {
        fs::remove_file(&source).ok()?;
        Some(format!("removed {}", self.relative(&source)))
      }
      false => {
        fs::write(&source, format!("export const {name} = {step};\n")).ok()?;
        Some(format!("created {}", self.relative(&source)))
      }
    }
  }

  fn change_manifest(&self, rng: &mut SplitMix) -> Option<String> {
    let repos = self.repos();
    match rng.below(3) {
      0 => {
        let tsconfigs: Vec<PathBuf> = repos.iter().map(|repo| repo.join("tsconfig.json")).filter(|path| path.exists()).collect();
        let path = rng.pick(&tsconfigs)?.clone();
        let mut config: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).ok()?).ok()?;
        let options = config.get_mut("compilerOptions")?.as_object_mut()?;
        let description = match options.contains_key("paths") {
          true => {
            options.remove("paths");
            options.remove("baseUrl");
            "removed paths"
          }
          false => {
            options.insert("baseUrl".to_owned(), serde_json::json!("."));
            options.insert("paths".to_owned(), serde_json::json!({ "@lib/*": ["src/common/*"] }));
            "added paths"
          }
        };
        fs::write(&path, serde_json::to_string_pretty(&config).ok()?).ok()?;
        Some(format!("{description} in {}", self.relative(&path)))
      }
      1 => {
        let cargo = repos.iter().map(|repo| repo.join("Cargo.toml")).find(|path| path.exists())?;
        let text = fs::read_to_string(&cargo).ok()?;
        let line = "anyhow = \"1\"\n";
        let (edited, description) = match text.contains(line) {
          true => (text.replace(line, ""), "removed anyhow"),
          false => (text.replacen("[dependencies]\n", &format!("[dependencies]\n{line}"), 1), "added anyhow"),
        };
        fs::write(&cargo, edited).ok()?;
        Some(format!("{description} in {}", self.relative(&cargo)))
      }
      _ => {
        let manifests: Vec<PathBuf> = repos.iter().map(|repo| repo.join("package.json")).filter(|path| path.exists()).collect();
        let path = rng.pick(&manifests)?.clone();
        let mut manifest: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).ok()?).ok()?;
        let object = manifest.as_object_mut()?;
        let dependencies = object.entry("dependencies").or_insert_with(|| serde_json::json!({})).as_object_mut()?;
        let description = match dependencies.remove("left-pad") {
          Some(_) => "removed left-pad",
          None => {
            dependencies.insert("left-pad".to_owned(), serde_json::json!("^1.3.0"));
            "added left-pad"
          }
        };
        fs::write(&path, serde_json::to_string_pretty(&manifest).ok()?).ok()?;
        Some(format!("{description} in {}", self.relative(&path)))
      }
    }
  }
}

fn relative_specifier(from_dir: &Path, target: &Path) -> Option<String> {
  let from: Vec<_> = from_dir.components().collect();
  let stripped = target.with_extension("");
  let to: Vec<_> = stripped.components().collect();
  let common = from.iter().zip(&to).take_while(|(left, right)| left == right).count();
  let mut parts: Vec<String> = (common..from.len()).map(|_| "..".to_owned()).collect();
  parts.extend(to[common..].iter().map(|component| component.as_os_str().to_string_lossy().into_owned()));
  let joined = parts.join("/");
  Some(match joined.starts_with("..") {
    true => joined,
    false => format!("./{joined}"),
  })
}
