use atlas_core::{EntityKey, KeyContext, Language, LanguageFamily, PackageContext, PackageEcosystem, RelPath, UNPACKAGED, ts_module_path};
use atlas_project::{Ecosystem, PackageGraph, PackageInfo};

pub(crate) fn clone_graph(graph: &PackageGraph) -> PackageGraph {
  PackageGraph {
    packages: graph.packages.clone(),
    workspaces: graph.workspaces.clone(),
    tsconfigs: graph.tsconfigs.clone(),
    workspace_dist: graph.workspace_dist.clone(),
    warnings: graph.warnings.clone(),
  }
}

pub(crate) fn same_graph(left: &PackageGraph, right: &PackageGraph) -> bool {
  left.packages == right.packages
    && left.workspaces == right.workspaces
    && left.tsconfigs == right.tsconfigs
    && left.workspace_dist == right.workspace_dist
    && left.warnings == right.warnings
}

pub(crate) fn graph_fingerprint(graph: &PackageGraph) -> String {
  blake3::hash(format!("{graph:?}").as_bytes()).to_hex().to_string()
}

pub(crate) fn default_context(graph: &PackageGraph, path: &RelPath) -> KeyContext {
  let family = Language::from_path(path.as_str()).map(Language::family);
  if family == Some(LanguageFamily::Rust) {
    let cargo = graph.package_for_file(path).and_then(PackageInfo::as_cargo);
    if let (Some(target), Some(cargo)) = (graph.rust_targets_for_file(path).first(), cargo) {
      return KeyContext {
        package: target.target.package_field(&cargo.name),
        module_path: target.module_path.clone(),
      };
    }
    let stem = path.as_str().strip_suffix(".rs").unwrap_or(path.as_str());
    return KeyContext {
      package: UNPACKAGED.to_owned(),
      module_path: stem.split('/').map(str::to_owned).collect(),
    };
  }
  let named = graph.package_for_file(path).and_then(|package| Some((package.name()?.to_owned(), package.root.clone())));
  let module_path = |dir: Option<&RelPath>| ts_module_path(path, dir).unwrap_or_else(|| path.segments().map(str::to_owned).collect());
  match named {
    Some((name, root)) => KeyContext {
      package: name,
      module_path: module_path(root.as_rel_path()),
    },
    None => KeyContext {
      package: UNPACKAGED.to_owned(),
      module_path: module_path(None),
    },
  }
}

fn ecosystem(package: &PackageInfo) -> PackageEcosystem {
  match package.ecosystem() {
    Ecosystem::Cargo => PackageEcosystem::Cargo,
    Ecosystem::Npm => PackageEcosystem::Npm,
  }
}

pub(crate) fn package_context(graph: &PackageGraph, path: &RelPath) -> Option<PackageContext> {
  let package = graph.package_for_file(path)?;
  let name = package.name()?.to_owned();
  let mut dependencies: Vec<String> = match (package.as_cargo(), package.as_npm()) {
    (Some(cargo), _) => cargo.dependencies.iter().map(|dependency| dependency.package_name.clone()).collect(),
    (_, Some(npm)) => npm.dependencies.iter().map(|dependency| dependency.name.clone()).collect(),
    _ => Vec::new(),
  };
  dependencies.sort();
  dependencies.dedup();
  Some(PackageContext {
    ecosystem: ecosystem(package),
    name,
    dependencies,
  })
}

pub(crate) fn package_key(package: &PackageInfo) -> Option<EntityKey> {
  Some(EntityKey::Package {
    ecosystem: ecosystem(package),
    name: package.name()?.to_owned(),
  })
}

pub(crate) fn package_key_for_file(graph: &PackageGraph, path: &RelPath) -> Option<EntityKey> {
  package_key(graph.package_for_file(path)?)
}
