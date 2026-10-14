use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::{AnalysisInput, ContentHash, FileRef, Language, LanguageAnalyzer, LanguageFamily, RelPath, RepoName};
use atlas_lang_ts::{ANALYZER_VERSION, TsAnalyzer};

const PINNED: (u32, &str, &str) = (
  4,
  "0ec51fa12b9612a96be0bdaf8f327a0fbfed6a3652a1ec4351cb90239ac53240",
  "99fe6e552170dba4dec7ed6dbb93736a6a9b31d4f88444718af7eaf52e0e07f7",
);

fn fixtures_root() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn walk(dir: &Path) -> Vec<PathBuf> {
  let mut files = Vec::new();
  let mut stack = vec![dir.to_path_buf()];
  while let Some(dir) = stack.pop() {
    for entry in fs::read_dir(&dir).unwrap().flatten() {
      match entry.file_type().unwrap().is_dir() {
        true => stack.push(entry.path()),
        false => files.push(entry.path()),
      }
    }
  }
  files.sort();
  files
}

fn relative(path: &Path, root: &Path) -> String {
  path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/")
}

fn walk_dirs(dir: &Path) -> Vec<PathBuf> {
  let mut dirs: Vec<PathBuf> = fs::read_dir(dir)
    .unwrap()
    .flatten()
    .filter(|entry| entry.file_type().unwrap().is_dir())
    .map(|entry| entry.path())
    .collect();
  dirs.sort();
  dirs
}

fn digests() -> (String, String) {
  let mut inputs = Vec::new();
  let mut output = Vec::new();
  let fixtures = walk_dirs(&fixtures_root()).into_iter().flat_map(|group| walk_dirs(&group));
  let corpus = walk_dirs(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pin-corpus"));
  for repo_dir in fixtures.chain(corpus) {
    let repo = RepoName::new(repo_dir.file_name().unwrap().to_string_lossy()).unwrap();
    for file in walk(&repo_dir) {
      let path = relative(&file, &repo_dir);
      if Language::from_path(&path).map(Language::family) != Some(LanguageFamily::Ts) {
        continue;
      }
      let source = fs::read_to_string(&file).unwrap();
      let file_ref = FileRef::detect(repo.clone(), RelPath::new(&path).unwrap(), ContentHash::of(source.as_bytes())).unwrap();
      let facts = TsAnalyzer::new().analyze(&AnalysisInput { file: &file_ref, source: &source }).unwrap();
      inputs.extend(serde_json::to_vec(&file_ref).unwrap());
      inputs.push(b'\n');
      output.extend(serde_json::to_vec(&facts).unwrap());
      output.push(b'\n');
    }
  }
  (ContentHash::of(&inputs).to_string(), ContentHash::of(&output).to_string())
}

fn assert_pinned(constant: &str, pinned: (u32, &str, &str), actual: (u32, String, String)) {
  let (version, inputs, output) = &actual;
  if (*version, inputs.as_str(), output.as_str()) == pinned {
    return;
  }
  let advice = match (inputs == pinned.1, *version == pinned.0) {
    (false, _) => format!("the analyzed fixture files changed, so re-pin; if this producer's code changed as well, also bump {constant}"),
    (true, true) => format!("the output over unchanged fixtures changed: bump {constant} so the engine re-analyses stored facts, then re-pin"),
    (true, false) => format!("{constant} changed: re-pin"),
  };
  panic!("{advice}\nnew pin: ({version}, \"{inputs}\", \"{output}\")");
}

#[test]
fn should_change_the_analyzer_output_over_the_fixtures_only_with_a_version_bump() {
  let (inputs, output) = digests();
  assert_pinned("atlas_lang_ts::ANALYZER_VERSION", PINNED, (ANALYZER_VERSION, inputs, output));
}
