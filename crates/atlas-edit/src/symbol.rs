use std::fs;
use std::ops::Range;

use atlas_core::ContentHash;

use crate::diff::{FileDiff, file_diff};
use crate::error::{EditError, EditResult};
use crate::indent::{FirstLine, clean_text, line_indent, reindent};
use crate::plan::{EditPlan, FileEdits, FileTarget, TextEdit};
use crate::region::{Lead, LeadKind, LeadingRegion, Syntax, lead, leading_region};
use crate::scope::EditScope;
use crate::text::{LineEnding, bom_len, decode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nesting {
  TopLevel,
  Nested,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolOp {
  ReplaceBody(String),
  InsertBefore(String),
  InsertAfter(String),
  SafeDelete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolSite<'a> {
  pub content: &'a str,
  pub span: Range<usize>,
  pub floor: usize,
  pub syntax: Syntax,
  pub nesting: Nesting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolTarget {
  pub file: FileTarget,
  pub expected: ContentHash,
  pub span: Range<usize>,
  pub floor: usize,
  pub syntax: Syntax,
  pub nesting: Nesting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolPlan {
  pub plan: EditPlan,
  pub diff: FileDiff,
  pub region: LeadingRegion,
}

pub fn plan_symbol_edit(scope: &EditScope, target: &SymbolTarget, op: &SymbolOp) -> EditResult<SymbolPlan> {
  let located = scope.locate(&target.file).map_err(|refusal| EditError::OutOfScope { refused: vec![refusal] })?;
  let bytes = fs::read(&located.absolute).map_err(|error| EditError::io("read", &located.absolute, error))?;
  if ContentHash::of(&bytes) != target.expected {
    return Err(EditError::Stale {
      paths: vec![located.file.to_string()],
    });
  }
  let content = decode(&located.file, &bytes)?;
  let site = SymbolSite {
    content,
    span: target.span.clone(),
    floor: target.floor,
    syntax: target.syntax,
    nesting: target.nesting,
  };
  let edit = symbol_edit(&site, op)?;
  let diff = file_diff(&located.file, content, std::slice::from_ref(&edit), &[])?;
  Ok(SymbolPlan {
    plan: EditPlan {
      files: vec![FileEdits::new(located.file, target.expected, vec![edit])],
      resources: Vec::new(),
    },
    diff,
    region: leading_region(content, target.span.start, target.syntax, target.floor),
  })
}

pub fn symbol_edit(site: &SymbolSite<'_>, op: &SymbolOp) -> EditResult<TextEdit> {
  let content = site.content;
  let span = &site.span;
  let bom = bom_len(content);
  if span.start > span.end || span.end > content.len() || span.start < bom || !content.is_char_boundary(span.start) || !content.is_char_boundary(span.end) {
    return Err(EditError::InvalidSpan {
      start: span.start,
      end: span.end,
      len: content.len(),
    });
  }
  let layout = Layout::of(site);
  let edit = match op {
    SymbolOp::ReplaceBody(text) => layout.replace_body(&cleaned(text)?)?,
    SymbolOp::InsertBefore(text) => layout.insert_before(&cleaned(text)?),
    SymbolOp::InsertAfter(text) => layout.insert_after(&cleaned(text)?),
    SymbolOp::SafeDelete => layout.delete()?,
  };
  Ok(TextEdit {
    new_text: LineEnding::dominant(content).normalize(&edit.new_text),
    ..edit
  })
}

fn cleaned(text: &str) -> EditResult<String> {
  let text = clean_text(text);
  if text.is_empty() { Err(EditError::EmptyText) } else { Ok(text) }
}

struct Layout<'a> {
  site: &'a SymbolSite<'a>,
  region: LeadingRegion,
  base: &'a str,
  bom: usize,
}

impl<'a> Layout<'a> {
  fn of(site: &'a SymbolSite<'a>) -> Self {
    Self {
      region: leading_region(site.content, site.span.start, site.syntax, site.floor),
      base: line_indent(site.content, site.span.start),
      bom: bom_len(site.content),
      site,
    }
  }

  fn content(&self) -> &'a str {
    self.site.content
  }

  fn replace_body(&self, text: &str) -> EditResult<TextEdit> {
    let span = &self.site.span;
    let syntax = self.site.syntax;
    let incoming = lead(text, syntax).ok_or(EditError::RegionUnparseable("the new text"))?;
    let supplies = |kind| incoming.lines.contains(&kind) || incoming.inline == Some(kind);
    let (supplies_docs, supplies_attributes) = (supplies(LeadKind::Doc), supplies(LeadKind::Attribute));
    let span_lead = lead(&self.content()[span.clone()], syntax).ok_or(EditError::RegionUnparseable("the declaration"))?;
    let body_start = span.start + span_lead.body_offset;
    if text[incoming.body_offset..].trim().is_empty() {
      return Err(EditError::EmptyText);
    }
    if !supplies_docs && !supplies_attributes && (incoming.lines.is_empty() || span_lead.inline.is_none()) {
      return Ok(TextEdit::replace(body_start..span.end, reindent(text, self.base, FirstLine::Bare)));
    }
    self.ensure_nothing_before()?;
    let existing_text = self.content()[self.region.start..span.end].replace("\r\n", "\n");
    let existing = lead(&existing_text, syntax).ok_or(EditError::RegionUnparseable("the leading docs and attributes"))?;
    let reindented = reindent(text, self.base, FirstLine::Indented);
    let incoming_lines: Vec<&str> = reindented.split('\n').collect();
    let rebuilt = Rebuild {
      existing_text: &existing_text,
      existing_lines: existing_text.split('\n').collect(),
      existing: &existing,
      incoming_lines,
      incoming: &incoming,
      supplies_docs,
      supplies_attributes,
    }
    .assemble();
    Ok(TextEdit::replace(self.region.start..span.end, rebuilt))
  }

  fn insert_before(&self, text: &str) -> TextEdit {
    let anchor = self.region.start;
    TextEdit::insert(anchor, format!("{}\n{}", reindent(text, self.base, FirstLine::Indented), self.separator()))
  }

  fn insert_after(&self, text: &str) -> TextEdit {
    let at = line_end(self.content(), self.last_offset());
    TextEdit::insert(at, format!("\n{}{}", self.separator(), reindent(text, self.base, FirstLine::Indented)))
  }

  fn delete(&self) -> EditResult<TextEdit> {
    self.ensure_nothing_before()?;
    let content = self.content();
    let last_line_end = line_end(content, self.last_offset());
    let trailing = &content[self.site.span.end.min(last_line_end)..last_line_end];
    if !is_trailing_noise(trailing) {
      return Err(EditError::NotLineAligned("code follows the declaration on its last line"));
    }
    let mut start = self.region.start;
    let mut end = next_line_start(content, last_line_end);
    let previous_blank = previous_line(content, start, self.bom).filter(|line| is_blank(content, line));
    let next_blank = next_line(content, end).filter(|line| is_blank(content, line));
    if let Some(next) = next_blank
      && (previous_blank.is_some() || start == self.bom)
    {
      end = next.end;
    } else if let Some(previous) = previous_blank
      && end == content.len()
    {
      start = previous.start;
    }
    Ok(TextEdit::delete(start..end))
  }

  fn ensure_nothing_before(&self) -> EditResult<()> {
    if self.content()[self.region.end..self.site.span.start].trim().is_empty() {
      return Ok(());
    }
    Err(EditError::NotLineAligned("code precedes the declaration on its first line"))
  }

  fn separator(&self) -> &'static str {
    let blank_above = previous_line(self.content(), self.region.start, self.bom).is_some_and(|line| is_blank(self.content(), &line));
    if self.site.nesting == Nesting::TopLevel || blank_above { "\n" } else { "" }
  }

  fn last_offset(&self) -> usize {
    let span = &self.site.span;
    let declaration = &self.content()[span.clone()];
    let trimmed = declaration.strip_suffix('\n').map_or(declaration, |line| line.strip_suffix('\r').unwrap_or(line));
    span.start + trimmed.len()
  }
}

struct Rebuild<'a> {
  existing_text: &'a str,
  existing_lines: Vec<&'a str>,
  existing: &'a Lead,
  incoming_lines: Vec<&'a str>,
  incoming: &'a Lead,
  supplies_docs: bool,
  supplies_attributes: bool,
}

impl Rebuild<'_> {
  fn assemble(&self) -> String {
    let incoming_of = |kind: LeadKind| -> Vec<&str> {
      self
        .incoming
        .lines
        .iter()
        .zip(&self.incoming_lines)
        .filter(|(line_kind, _)| **line_kind == kind)
        .map(|(_, line)| *line)
        .collect()
    };
    let existing_plain: Vec<&str> = self.existing_lead().filter(|(kind, _)| *kind == LeadKind::Plain).map(|(_, line)| line.trim()).collect();
    let has_existing_docs = self.existing.lines.contains(&LeadKind::Doc);
    let mut output: Vec<&str> = Vec::new();
    let (mut docs_written, mut attributes_written) = (false, false);
    for (kind, line) in self.existing_lead() {
      match kind {
        LeadKind::Doc if self.supplies_docs => {
          if !docs_written {
            output.extend(incoming_of(LeadKind::Doc));
            docs_written = true;
          }
        }
        LeadKind::Attribute => {
          if self.supplies_docs && !has_existing_docs && !docs_written {
            output.extend(incoming_of(LeadKind::Doc));
            docs_written = true;
          }
          if !self.supplies_attributes {
            output.push(line);
          } else if !attributes_written {
            output.extend(incoming_of(LeadKind::Attribute));
            attributes_written = true;
          }
        }
        _ => output.push(line),
      }
    }
    if self.supplies_docs && !docs_written {
      output.extend(incoming_of(LeadKind::Doc));
    }
    if self.supplies_attributes && !attributes_written {
      output.extend(incoming_of(LeadKind::Attribute));
    }
    output.extend(incoming_of(LeadKind::Plain).into_iter().filter(|line| !existing_plain.contains(&line.trim())));
    let mut assembled: String = output.iter().map(|line| format!("{line}\n")).collect();
    let body = &self.incoming_lines[self.incoming.lines.len()..];
    let keep_inline = match self.existing.inline {
      Some(LeadKind::Attribute) => !self.supplies_attributes,
      Some(LeadKind::Doc) => !self.supplies_docs,
      Some(LeadKind::Plain) => true,
      None => false,
    };
    if keep_inline {
      let body_line_start: usize = self.existing_lines[..self.existing.lines.len()].iter().map(|line| line.len() + 1).sum();
      assembled.push_str(&self.existing_text[body_line_start..self.existing.body_offset]);
      assembled.push_str(body[0].trim_start());
      for line in &body[1..] {
        assembled.push('\n');
        assembled.push_str(line);
      }
    } else {
      assembled.push_str(&body.join("\n"));
    }
    assembled
  }

  fn existing_lead(&self) -> impl Iterator<Item = (LeadKind, &str)> {
    self.existing.lines.iter().copied().zip(self.existing_lines.iter().copied())
  }
}

fn is_trailing_noise(text: &str) -> bool {
  let rest = text.trim_start();
  let rest = rest.strip_prefix([';', ',']).unwrap_or(rest).trim_start();
  rest.is_empty() || rest.starts_with("//")
}

fn line_end(content: &str, offset: usize) -> usize {
  let Some(found) = content[offset..].find('\n') else { return content.len() };
  let newline = offset + found;
  if newline > offset && content.as_bytes()[newline - 1] == b'\r' {
    newline - 1
  } else {
    newline
  }
}

fn next_line_start(content: &str, line_end: usize) -> usize {
  content[line_end..].find('\n').map_or(content.len(), |found| line_end + found + 1)
}

fn previous_line(content: &str, line_start: usize, bom: usize) -> Option<Range<usize>> {
  if line_start <= bom {
    return None;
  }
  let start = content[..line_start - 1].rfind('\n').map_or(bom, |newline| newline + 1);
  Some(start..line_start)
}

fn next_line(content: &str, line_start: usize) -> Option<Range<usize>> {
  if line_start >= content.len() {
    return None;
  }
  Some(line_start..next_line_start(content, line_start))
}

fn is_blank(content: &str, line: &Range<usize>) -> bool {
  content[line.clone()].trim().is_empty()
}
