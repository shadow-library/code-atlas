use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::{AnalysisInput, ContentHash, DetectInput, FileFacts, FileRef, Language, LanguageAnalyzer, LanguageFamily, RelPath, RepoName};
use atlas_detect::DetectorRegistry;
use atlas_lang_rust::RustAnalyzer;
use atlas_lang_ts::TsAnalyzer;

const PINNED_INPUTS: &str = "01d9bc1736f229a24cbcc0ebef8826b22666daed6a066113aa0674bbae79226a";
const PINNED: [(&str, u32, &str); 10] = [
  ("auth-schemes", 1, "7fca69b6290a574556ceaee0946162a0138b762ace77390b0d597194d51f0935"),
  ("config-env", 2, "fff96a6ccd0aebd31d24a002b37d3c2aaa231a7d43f1a14192c88b4dc0ef9e2a"),
  ("db", 1, "3d36016b9fcefda2574f5e620eb5237fdc84881e3715eafc41f0bb84fefc627c"),
  ("events", 1, "56ea15b0eee70175cbc18b834b599a38e9d4e84b2913700bf6c5d5023d568560"),
  ("http-clients", 1, "a12672b4fbbecaf3dd70b86b67febb8c2e7a505436ccd34ddc5409736e3644ab"),
  ("http-endpoints-axum", 1, "4e8073d521653481c3f481efd33b1f32b71341b97bb752d18751ac9fd016c260"),
  ("http-endpoints-express", 1, "633c6c825f8b3d613b9994233ebe2f768f78fbb084734a2e3c975139ebb05626"),
  ("http-endpoints-nest", 1, "9aa94679e187c47892e60eebaaf27b03ae68061e6db75a26e631acfefb4af39e"),
  ("jobs", 1, "18c3f1518499074939d9a52840f8d6332c307ee892d6f63087cf846055add3a7"),
  ("tests", 1, "b96631eda220c21cecd1a6f43c253136236f8b2e95475c1ab1658924a3ed27d4"),
];

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

fn relative(path: &Path, root: &Path) -> String {
  path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/")
}

struct Analyzed {
  facts: FileFacts,
  source: String,
}

fn analyzed_fixtures() -> Vec<Analyzed> {
  let mut analyzed = Vec::new();
  let fixtures = walk_dirs(&fixtures_root()).into_iter().flat_map(|group| walk_dirs(&group));
  let corpus = walk_dirs(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pin-corpus"));
  for repo_dir in fixtures.chain(corpus) {
    let repo = RepoName::new(repo_dir.file_name().unwrap().to_string_lossy()).unwrap();
    for file in walk(&repo_dir) {
      let path = relative(&file, &repo_dir);
      let Some(language) = Language::from_path(&path) else { continue };
      let source = fs::read_to_string(&file).unwrap();
      let file_ref = FileRef::detect(repo.clone(), RelPath::new(&path).unwrap(), ContentHash::of(source.as_bytes())).unwrap();
      let input = AnalysisInput { file: &file_ref, source: &source };
      let facts = match language.family() {
        LanguageFamily::Rust => RustAnalyzer::new().analyze(&input),
        _ => TsAnalyzer.analyze(&input),
      }
      .unwrap();
      analyzed.push(Analyzed { facts, source });
    }
  }
  analyzed
}

fn inputs_digest(analyzed: &[Analyzed]) -> String {
  let mut bytes = Vec::new();
  for file in analyzed {
    bytes.extend(serde_json::to_vec(&file.facts).unwrap());
    bytes.push(b'\n');
  }
  ContentHash::of(&bytes).to_string()
}

fn output_digests(analyzed: &[Analyzed]) -> BTreeMap<&'static str, (u32, String)> {
  let registry = DetectorRegistry::default();
  let mut outputs: BTreeMap<&'static str, (u32, Vec<u8>)> = registry
    .ids()
    .into_iter()
    .zip(registry.producers())
    .map(|(id, producer)| (id, (producer.version, Vec::new())))
    .collect();
  for file in analyzed {
    let input = DetectInput {
      facts: &file.facts,
      source: &file.source,
      package: None,
    };
    for run in registry.run_each(&input) {
      let mut diagnostics: Vec<String> = run.diagnostics.iter().map(|diagnostic| serde_json::to_string(diagnostic).unwrap()).collect();
      diagnostics.sort();
      let bytes = &mut outputs.get_mut(run.detector).unwrap().1;
      bytes.extend(file.facts.file.repo.as_str().as_bytes());
      bytes.push(0);
      bytes.extend(file.facts.file.path.as_str().as_bytes());
      bytes.push(0);
      bytes.extend(serde_json::to_vec(&run.detection).unwrap());
      bytes.extend(serde_json::to_vec(&diagnostics).unwrap());
      bytes.push(b'\n');
    }
  }
  outputs
    .into_iter()
    .map(|(id, (version, bytes))| (id, (version, ContentHash::of(&bytes).to_string())))
    .collect()
}

#[test]
fn should_change_each_detector_output_over_the_fixtures_only_with_a_version_bump() {
  let analyzed = analyzed_fixtures();
  let inputs = inputs_digest(&analyzed);
  let outputs = output_digests(&analyzed);
  let actual: Vec<(&str, u32, &str)> = outputs.iter().map(|(id, (version, digest))| (*id, *version, digest.as_str())).collect();
  if (inputs.as_str(), actual.as_slice()) == (PINNED_INPUTS, PINNED.as_slice()) {
    return;
  }
  let pinned: BTreeMap<&str, (u32, &str)> = PINNED.iter().map(|(id, version, digest)| (*id, (*version, *digest))).collect();
  let advice: Vec<String> = match inputs == PINNED_INPUTS {
    false => vec![
      "the analyzed fixture or pin-corpus files, or the analyzers, changed, so re-pin; bump a detector's version in its `detector!` line only if its own code changed as well"
        .to_owned(),
    ],
    true => actual
      .iter()
      .filter(|(id, version, digest)| pinned.get(id) != Some(&(*version, *digest)))
      .map(|(id, version, _)| match pinned.get(id).map(|(pinned, _)| pinned) {
        Some(pinned) if pinned == version => {
          format!("{id}: its output over unchanged inputs changed: bump its version in its `detector!` line so the engine re-analyses stored files, then re-pin")
        }
        _ => format!("{id}: its version changed: re-pin"),
      })
      .collect(),
  };
  panic!("{}\nnew pins:\nconst PINNED_INPUTS: &str = \"{inputs}\";\n{actual:#?}", advice.join("\n"));
}
