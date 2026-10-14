use crate::graph::group_by_repo;
use crate::model::{Location, UnresolvedSite};
use crate::queries::{FindAnswer, GraphReference, GraphReferences, HierarchyAnswer, HierarchyNode, OutlineAnswer, OutlineParse, ReferenceSide, SymbolNode};
use crate::text::{RenderText, TextOut, caveats, entity_head, entity_line, omitted, snippet};

fn signed(head: String, signature: Option<&str>) -> String {
  match signature {
    Some(signature) => format!("{head}  {signature}"),
    None => head,
  }
}

fn node_place(node: &SymbolNode, file: Option<&Location>) -> String {
  match (&node.entity.location, file) {
    (Some(location), Some(file)) if location.repo == file.repo && location.path == file.path => location.line.map_or_else(String::new, |line| format!(":{line}")),
    (Some(location), _) => location.to_string(),
    (None, _) => String::new(),
  }
}

fn nodes(out: &mut TextOut, indent: usize, nodes: &[SymbolNode], file: Option<&Location>) {
  for node in nodes {
    let mut head = format!("{} {} {}", node.name, node.entity.kind, node_place(node, file));
    if let Some(tag) = node.entity.tag() {
      head = format!("{head} {tag}");
    }
    out.line(indent, signed(head, node.signature.as_deref()));
    self::nodes(out, indent + 1, &node.children, file);
  }
}

fn matches(total: usize) -> String {
  match total {
    0 => "no matches".to_owned(),
    1 => "1 match".to_owned(),
    many => format!("{many} matches"),
  }
}

impl RenderText for FindAnswer {
  fn render_text(&self, out: &mut TextOut) {
    out.line(0, format!("find {:?}: {}", self.pattern, matches(self.matches.total)));
    for found in &self.matches.items {
      let mut head = format!("{} {}", found.name_path, found.entity.kind);
      if let Some(location) = &found.entity.location {
        head = format!("{head} {location}");
      }
      if let Some(tag) = found.entity.tag() {
        head = format!("{head} {tag}");
      }
      out.line(1, signed(head, found.signature.as_deref()));
      nodes(out, 2, &found.children, found.entity.location.as_ref());
      if let Some(body) = &found.body {
        snippet(out, 2, body);
      }
    }
    omitted(out, 1, self.matches.omitted());
    caveats(out, &self.caveats);
  }
}

impl RenderText for OutlineAnswer {
  fn render_text(&self, out: &mut TextOut) {
    out.line(
      0,
      format!(
        "outline {} source=graph (from the index, syntactic; for the exact language-server outline use source=lsp)",
        self.file
      ),
    );
    match &self.parse {
      OutlineParse::Complete => {}
      OutlineParse::Partial { error_count } => out.line(0, format!("PARTIAL: the file parsed with {error_count} errors; symbols near the errors may be missing")),
      OutlineParse::Failed { reason } => {
        out.line(
          0,
          format!("NO FACTS: the file failed to parse ({reason}), so the graph has no symbols for it; use source=lsp"),
        );
        return caveats(out, &self.caveats);
      }
    }
    if self.symbols.is_empty() {
      out.line(1, "(no symbols)");
    }
    nodes(out, 1, &self.symbols, Some(&self.file));
    caveats(out, &self.caveats);
  }
}

fn reference_line(reference: &GraphReference) -> String {
  let site = match &reference.enclosing {
    Some(enclosing) => enclosing.tagged_short(),
    None if reference.location.is_none() => reference.other.tagged_short(),
    None => "(file)".to_owned(),
  };
  let place = match &reference.location {
    Some(location) => match (location.line, location.column) {
      (Some(line), Some(column)) => format!("{}:{line}:{column}", location.path),
      (Some(line), None) => format!("{}:{line}", location.path),
      (None, _) => location.path.clone(),
    },
    None => "(no evidence)".to_owned(),
  };
  let relation = match reference.side {
    ReferenceSide::Heritage => format!("{} {}", reference.phrase, reference.other.short),
    ReferenceSide::Inbound | ReferenceSide::Implemented => reference.phrase.clone(),
  };
  let mut line = format!("{site}  {place}  {relation} {}", reference.certainty.tag);
  if let Some(overload) = reference.overload {
    line = format!("{line} overload [{overload}]");
  }
  if let Some(mock) = &reference.mock {
    line = format!("{line} mock={mock}");
  }
  match &reference.snippet {
    Some(snippet) => format!("{line}  {snippet}"),
    None => line,
  }
}

fn reference_repo(reference: &GraphReference) -> String {
  match &reference.location {
    Some(location) => location.repo.clone(),
    None => reference.other.repo().to_owned(),
  }
}

fn unresolved_line(site: &UnresolvedSite) -> String {
  format!("{} {} ({})", site.location, site.path, site.reason)
}

impl RenderText for GraphReferences {
  fn render_text(&self, out: &mut TextOut) {
    out.line(0, format!("references {} source=graph", entity_line(&self.target)));
    for (index, overload) in self.overloads.iter().enumerate() {
      out.line(1, format!("overload [{index}] {}", entity_line(overload)));
    }
    if self.references.is_empty() {
      out.line(0, "(none)");
    }
    for (repo, references) in group_by_repo(self.references.iter().collect(), |reference| reference_repo(reference)) {
      out.line(0, format!("{repo} ({})", references.len()));
      for reference in references {
        out.line(1, reference_line(reference));
      }
    }
    if !self.unresolved.is_empty() {
      out.line(0, format!("unresolved references with the same name ({}):", self.unresolved.len()));
      for site in &self.unresolved {
        out.line(1, unresolved_line(site));
      }
    }
    if !self.unverified_files.is_empty() {
      out.line(0, "changed since indexing (line only, no columns):");
      for file in &self.unverified_files {
        out.line(1, file.to_string());
      }
    }
    caveats(out, &self.caveats);
  }
}

fn hierarchy_nodes(out: &mut TextOut, indent: usize, nodes: &[HierarchyNode]) {
  for node in nodes {
    let mut line = format!("{} {} {}", node.phrase, entity_head(&node.entity), node.certainty.tag);
    if let Some(via) = &node.via {
      line = format!("{line} via {via}");
    }
    out.line(indent, line);
    hierarchy_nodes(out, indent + 1, &node.children);
  }
}

impl RenderText for HierarchyAnswer {
  fn render_text(&self, out: &mut TextOut) {
    out.line(0, format!("hierarchy {}", entity_line(&self.target)));
    for (label, nodes) in [("supertypes", &self.supertypes), ("subtypes", &self.subtypes)] {
      out.line(0, format!("{label}:"));
      if nodes.is_empty() {
        out.line(1, "(none)");
      }
      hierarchy_nodes(out, 1, nodes);
    }
    caveats(out, &self.caveats);
  }
}
