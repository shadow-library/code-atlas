use crate::queries::{AnchorView, DocKind, DocPointers, DocSection, DocsAnswer, FileNotesAnswer, MentionView, NotePointer, NoteView, NotesAnswer};
use crate::text::{RenderText, TextOut, caveats, counted, entity_line};

pub(crate) const NOTES: &str = "NOTES";
pub(crate) const DOCS: &str = "DOCS";
const ANCHORS_SHOWN: usize = 3;
const DRIFT: &str = "names nothing in the index (renamed, removed or outside the project)";

fn kind_label(section: &DocSection) -> String {
  match (section.kind, section.note_status) {
    (DocKind::Note, Some(status)) => format!("note {}", status.as_str()),
    (DocKind::Note, None) => "note".to_owned(),
    (DocKind::Doc, _) => "doc".to_owned(),
  }
}

fn lines_label(start: u32, end: u32) -> String {
  match start == end {
    true => start.to_string(),
    false => format!("{start}-{end}"),
  }
}

fn mention_line(mention: &MentionView) -> String {
  match mention {
    MentionView::Resolved {
      text,
      certainty,
      candidates,
      total,
      ..
    } => {
      let shown: Vec<String> = candidates.iter().map(entity_line).collect();
      let more = match total.saturating_sub(candidates.len()) {
        0 => String::new(),
        hidden => format!(" (+{hidden} more)"),
      };
      format!("`{text}` -> {}{more} {}", shown.join(" | "), certainty.tag)
    }
    MentionView::Drift { text, line } => format!("`{text}` (line {line}) {DRIFT}"),
  }
}

pub(crate) fn doc_section_lines(out: &mut TextOut, indent: usize, section: &DocSection) {
  out.line(
    indent,
    format!(
      "{}:{} > {} {} [{}]",
      section.repo,
      section.path,
      section.heading,
      lines_label(section.start_line, section.end_line),
      kind_label(section)
    ),
  );
  out.line(indent + 1, format!("file: {}", section.absolute_path));
  section_body(out, indent + 1, section);
}

pub(crate) fn fused_section_lines(out: &mut TextOut, indent: usize, section: &DocSection) {
  out.line(
    indent,
    format!(
      "{}:{} > {} [{}]",
      section.absolute_path,
      lines_label(section.start_line, section.end_line),
      section.heading,
      kind_label(section)
    ),
  );
  section_body(out, indent + 1, section);
}

fn section_body(out: &mut TextOut, indent: usize, section: &DocSection) {
  if !section.snippet.is_empty() {
    out.line(indent, &section.snippet);
  }
  for mention in &section.mentions {
    out.line(indent, format!("mentions {}", mention_line(mention)));
  }
}

impl RenderText for DocsAnswer {
  fn render_text(&self, out: &mut TextOut) {
    let noun = if self.notes_only { "notes" } else { "docs" };
    let subject = match (&self.query, &self.about) {
      (Some(query), None) => format!("{noun} matching {query:?}"),
      (None, Some(about)) => format!("{noun} that mention {}", entity_line(about)),
      (Some(query), Some(about)) => format!("{noun} that mention {} and match {query:?}", entity_line(about)),
      (None, None) => noun.to_owned(),
    };
    out.line(0, format!("{subject} ({} of {})", self.sections.len(), counted(self.total, "section")));
    if self.sections.is_empty() {
      out.line(1, "none: no section of a repository doc or note matches");
    }
    for section in &self.sections {
      doc_section_lines(out, 1, section);
    }
    caveats(out, &self.caveats);
  }
}

fn anchor_line(anchor: &AnchorView) -> String {
  let mut line = format!("anchor {} [{}]", anchor.target, anchor.status.as_str());
  if let Some(entity) = &anchor.entity {
    line.push_str(&format!(" -> {}", entity_line(entity)));
  }
  if let Some(certainty) = &anchor.certainty {
    line.push_str(&format!(" (from a mention {})", certainty.tag));
  }
  line
}

pub(crate) fn note_lines(out: &mut TextOut, indent: usize, note: &NoteView) {
  out.line(indent, format!("{}:{} {:?} [{}]", note.repo, note.path, note.title, note.status.as_str()));
  out.line(indent + 1, format!("file: {}", note.absolute_path));
  for anchor in &note.anchors {
    out.line(indent + 1, anchor_line(anchor));
  }
}

impl RenderText for NotesAnswer {
  fn render_text(&self, out: &mut TextOut) {
    let title = if self.stale_only { "stale notes" } else { "notes" };
    out.line(0, format!("{title} ({})", self.notes.len()));
    if self.notes.is_empty() {
      let none = match self.stale_only {
        true => "none: every note under .code-atlas/notes/ matches the code it is anchored to",
        false => "none: no repo of the project has notes under .code-atlas/notes/",
      };
      out.line(1, none);
    }
    for note in &self.notes {
      note_lines(out, 1, note);
    }
    caveats(out, &self.caveats);
  }
}

impl RenderText for FileNotesAnswer {
  fn render_text(&self, out: &mut TextOut) {
    out.line(0, format!("notes anchored to {}:{} ({})", self.file.repo, self.file.path, self.notes.len()));
    for anchored in &self.notes {
      note_lines(out, 1, &anchored.note);
      for section in &anchored.sections {
        out.line(2, format!("{} (line {}): {}", section.heading, section.start_line, section.text));
      }
    }
    caveats(out, &self.caveats);
  }
}

pub(crate) fn note_pointer_line(note: &NotePointer) -> String {
  let tag = note.certainty.as_ref().map(|certainty| format!(" {}", certainty.tag)).unwrap_or_default();
  let shown: Vec<&str> = note.anchored_to.iter().take(ANCHORS_SHOWN).map(String::as_str).collect();
  let more = match note.anchored_to.len().saturating_sub(ANCHORS_SHOWN) {
    0 => String::new(),
    hidden => format!(" (+{hidden})"),
  };
  format!(
    "{}:{} > {} [{}] on {}{more}{tag}",
    note.repo,
    note.path,
    note.heading,
    note.status.as_str(),
    shown.join(", ")
  )
}

pub(crate) fn pointer_sections(out: &mut TextOut, pointers: &DocPointers) {
  if !pointers.notes.is_empty() {
    out.line(0, format!("{NOTES} ({} of {}): notes anchored to it", pointers.notes.len(), pointers.notes_total));
    for note in &pointers.notes {
      out.line(1, note_pointer_line(note));
    }
  }
  if !pointers.docs.is_empty() {
    out.line(0, format!("{DOCS} ({} of {}): doc sections that mention it", pointers.docs.len(), pointers.docs_total));
    for doc in &pointers.docs {
      out.line(1, format!("{}:{}:{} > {} {}", doc.repo, doc.path, doc.start_line, doc.heading, doc.certainty.tag));
    }
  }
}
