use std::ops::Range;

const MIN_MENTION_CHARS: usize = 3;
const MAX_MENTION_CHARS: usize = 80;
const FENCE_MIN: usize = 3;
const MAX_HEADING_LEVEL: usize = 6;
const FRONTMATTER_DELIMITER: &str = "---";
const ANCHORS_KEY: &str = "anchors:";
const SYMBOL_KEY: &str = "symbol";
const BODY_KEY: &str = "body";
const BOM: char = '\u{feff}';
const FILE_EXTENSIONS: [&str; 24] = [
  "md", "mdx", "json", "ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts", "rs", "toml", "yaml", "yml", "lock", "txt", "sh", "css", "html", "sql", "env", "py", "go",
];
const FENCED_KEYWORDS: [&str; 40] = [
  "const",
  "let",
  "var",
  "function",
  "return",
  "await",
  "async",
  "export",
  "import",
  "from",
  "class",
  "interface",
  "type",
  "new",
  "this",
  "true",
  "false",
  "null",
  "undefined",
  "pub",
  "use",
  "mod",
  "impl",
  "struct",
  "enum",
  "trait",
  "self",
  "Self",
  "mut",
  "match",
  "for",
  "while",
  "loop",
  "if",
  "else",
  "fn",
  "crate",
  "super",
  "where",
  "static",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedDoc {
  pub frontmatter: Option<Frontmatter>,
  pub sections: Vec<RawSection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawSection {
  pub heading: Vec<String>,
  pub preamble: bool,
  pub start_line: u32,
  pub end_line: u32,
  pub body: String,
  pub mentions: Vec<RawMention>,
}

impl RawSection {
  pub fn title(&self) -> &str {
    self.heading.last().map_or("", String::as_str)
  }

  pub fn heading_path(&self) -> String {
    self.heading.join(" > ")
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
  Code,
  Fenced,
  Link,
  Prose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
  Path,
  Name { drift_eligible: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawMention {
  pub text: String,
  pub line: u32,
  pub origin: Origin,
  pub shape: Shape,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Frontmatter {
  pub anchors: Vec<RawAnchor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawAnchor {
  pub target: String,
  pub body: Option<String>,
  pub line: u32,
  pub body_value: Option<Range<usize>>,
  pub insert_at: usize,
  pub indent: usize,
}

struct Line<'t> {
  number: u32,
  start: usize,
  text: &'t str,
}

fn lines_of(text: &str, from: usize) -> Vec<Line<'_>> {
  let mut lines = Vec::new();
  let mut start = 0;
  for (index, raw) in text.split_inclusive('\n').enumerate() {
    let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
    if start + raw.len() > from {
      let skip = from.saturating_sub(start);
      lines.push(Line {
        number,
        start: start + skip,
        text: raw[skip..].trim_end_matches(['\n', '\r']),
      });
    }
    start += raw.len();
  }
  lines
}

pub(crate) fn parse(text: &str, file_name: &str) -> ParsedDoc {
  let bom = if text.starts_with(BOM) { BOM.len_utf8() } else { 0 };
  let (frontmatter, body_start) = match frontmatter(text, bom) {
    Some((frontmatter, end)) => (Some(frontmatter), end),
    None => (None, bom),
  };
  ParsedDoc {
    frontmatter,
    sections: sections(text, body_start, file_name),
  }
}

pub(crate) fn bom_len(text: &str) -> usize {
  if text.starts_with(BOM) { BOM.len_utf8() } else { 0 }
}

fn frontmatter(text: &str, bom: usize) -> Option<(Frontmatter, usize)> {
  let lines = lines_of(text, bom);
  let (first, rest) = lines.split_first()?;
  if first.text.trim_end() != FRONTMATTER_DELIMITER || first.start != bom {
    return None;
  }
  let closing = rest.iter().position(|line| matches!(line.text.trim_end(), "---" | "..."))?;
  let inner = &rest[..closing];
  let end = rest.get(closing + 1).map_or(text.len(), |line| line.start);
  Some((Frontmatter { anchors: anchors(text, inner) }, end))
}

fn indent_of(text: &str) -> usize {
  text.len() - text.trim_start_matches([' ', '\t']).len()
}

fn line_end(text: &str, line: &Line<'_>) -> usize {
  let after = line.start + line.text.len();
  match text[after..].find('\n') {
    Some(offset) => after + offset + 1,
    None => text.len(),
  }
}

fn anchors(text: &str, lines: &[Line<'_>]) -> Vec<RawAnchor> {
  let Some(key) = lines.iter().position(|line| line.text.trim_end() == ANCHORS_KEY) else {
    return Vec::new();
  };
  let mut anchors: Vec<RawAnchor> = Vec::new();
  let mut current: Option<RawAnchor> = None;
  for line in &lines[key + 1..] {
    let trimmed = line.text.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
      continue;
    }
    let indent = indent_of(line.text);
    let offset = match trimmed.strip_prefix('-') {
      Some(rest) if rest.is_empty() || rest.starts_with([' ', '\t']) => {
        let offset = indent + 1 + indent_of(rest);
        anchors.extend(current.take());
        current = Some(RawAnchor {
          target: String::new(),
          body: None,
          line: line.number,
          body_value: None,
          insert_at: line_end(text, line),
          indent: offset,
        });
        offset
      }
      _ if indent == 0 => break,
      _ if current.as_ref().is_some_and(|anchor| indent >= anchor.indent) => indent,
      _ => continue,
    };
    let Some(anchor) = current.as_mut() else { continue };
    anchor.insert_at = line_end(text, line);
    let entry = &line.text[offset..];
    let Some((name, raw)) = entry.split_once(':') else { continue };
    let (value, range) = scalar(raw, line.start + offset + name.len() + 1);
    match name.trim() {
      SYMBOL_KEY => {
        anchor.target = value;
        anchor.line = line.number;
      }
      BODY_KEY => {
        anchor.body = Some(value).filter(|value| !value.is_empty());
        anchor.body_value = Some(range);
      }
      _ => {}
    }
  }
  anchors.extend(current);
  anchors.retain(|anchor| !anchor.target.is_empty());
  anchors
}

fn scalar(raw: &str, start: usize) -> (String, Range<usize>) {
  let leading = indent_of(raw);
  let content = &raw[leading..];
  let quoted = content.starts_with(['"', '\'']);
  let end = match quoted {
    true => content[1..].find(content.chars().next().unwrap_or('"')).map_or(content.len(), |close| close + 2),
    false => content.find(" #").or_else(|| content.find("\t#")).unwrap_or(content.len()),
  };
  let token = content[..end].trim_end();
  let value = match quoted {
    true => token.get(1..token.len().saturating_sub(1)).unwrap_or_default().to_owned(),
    false => token.to_owned(),
  };
  let from = start + leading;
  (value, from..from + token.len())
}

struct Fence {
  marker: char,
  length: usize,
}

fn fence_of(text: &str) -> Option<Fence> {
  let indent = indent_of(text);
  if indent > 3 {
    return None;
  }
  let rest = &text[indent..];
  let marker = rest.chars().next().filter(|marker| matches!(marker, '`' | '~'))?;
  let length = rest.chars().take_while(|character| *character == marker).count();
  if length < FENCE_MIN || (marker == '`' && rest[length..].contains('`')) {
    return None;
  }
  Some(Fence { marker, length })
}

fn closes(fence: &Fence, text: &str) -> bool {
  let indent = indent_of(text);
  if indent > 3 {
    return false;
  }
  let rest = &text[indent..];
  let length = rest.chars().take_while(|character| *character == fence.marker).count();
  length >= fence.length && rest[length * fence.marker.len_utf8()..].trim().is_empty()
}

fn heading_of(text: &str) -> Option<(usize, String)> {
  let indent = indent_of(text);
  if indent > 3 || text[..indent].contains('\t') {
    return None;
  }
  let rest = &text[indent..];
  let level = rest.chars().take_while(|character| *character == '#').count();
  if level == 0 || level > MAX_HEADING_LEVEL {
    return None;
  }
  let after = &rest[level..];
  if !after.is_empty() && !after.starts_with([' ', '\t']) {
    return None;
  }
  let mut title = after.trim();
  let closing = title.trim_end_matches('#');
  if closing.len() != title.len() && (closing.is_empty() || closing.ends_with([' ', '\t'])) {
    title = closing.trim_end();
  }
  Some((level, title.to_owned()))
}

struct Open {
  levels: Vec<(usize, String)>,
  start_line: u32,
  end_line: u32,
  body: Vec<String>,
  mentions: Vec<RawMention>,
}

impl Open {
  fn close(self) -> Option<RawSection> {
    let named = !self.levels.is_empty();
    let blank = self.body.iter().all(|line| line.trim().is_empty());
    if !named && blank {
      return None;
    }
    Some(RawSection {
      heading: self.levels.into_iter().map(|(_, title)| title).collect(),
      preamble: !named,
      start_line: self.start_line,
      end_line: self.end_line,
      body: self.body.join("\n").trim_matches('\n').to_owned(),
      mentions: self.mentions,
    })
  }
}

fn sections(text: &str, from: usize, file_name: &str) -> Vec<RawSection> {
  let lines = lines_of(text, from);
  let mut done = Vec::new();
  let first_line = lines.first().map_or(1, |line| line.number);
  let mut open = Open {
    levels: Vec::new(),
    start_line: first_line,
    end_line: first_line,
    body: Vec::new(),
    mentions: Vec::new(),
  };
  let mut fence: Option<Fence> = None;
  for line in &lines {
    if let Some(current) = &fence {
      if closes(current, line.text) {
        fence = None;
      } else {
        open.mentions.extend(fenced_mentions(line.text, line.number));
      }
      open.body.push(line.text.to_owned());
      open.end_line = line.number;
      continue;
    }
    if let Some(found) = fence_of(line.text) {
      fence = Some(found);
      open.body.push(line.text.to_owned());
      open.end_line = line.number;
      continue;
    }
    if let Some((level, title)) = heading_of(line.text) {
      let mut levels = open.levels.clone();
      levels.retain(|(kept, _)| *kept < level);
      levels.push((level, title));
      let previous = std::mem::replace(
        &mut open,
        Open {
          levels,
          start_line: line.number,
          end_line: line.number,
          body: Vec::new(),
          mentions: Vec::new(),
        },
      );
      done.extend(previous.close());
      continue;
    }
    open.mentions.extend(prose_mentions(line.text, line.number));
    open.body.push(line.text.to_owned());
    open.end_line = line.number;
  }
  done.extend(open.close());
  for section in done.iter_mut().filter(|section| section.heading.is_empty()) {
    section.heading = vec![file_name.to_owned()];
  }
  done
}

fn is_identifier_start(character: char) -> bool {
  character.is_ascii_alphabetic() || character == '_' || character == '$'
}

fn is_identifier_part(character: char) -> bool {
  character.is_ascii_alphanumeric() || character == '_' || character == '$'
}

fn dotted_name(text: &str) -> bool {
  text.split('.').all(|segment| {
    let mut characters = segment.chars();
    characters.next().is_some_and(is_identifier_start) && characters.all(is_identifier_part)
  })
}

fn rust_path(text: &str) -> bool {
  let segments: Vec<&str> = text.split("::").collect();
  segments.len() > 1
    && segments.iter().all(|segment| {
      let mut characters = segment.chars();
      characters.next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_') && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
    })
}

pub(crate) fn is_name_like(text: &str) -> bool {
  (MIN_MENTION_CHARS..=MAX_MENTION_CHARS).contains(&text.len()) && (dotted_name(text) || rust_path(text))
}

fn has_file_extension(text: &str) -> bool {
  let name = text.rsplit('/').next().unwrap_or(text);
  name
    .rsplit_once('.')
    .is_some_and(|(stem, extension)| !stem.is_empty() && FILE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
}

fn is_camel(text: &str) -> bool {
  let characters: Vec<char> = text.chars().collect();
  let hump = characters.windows(2).any(|pair| pair[0].is_ascii_lowercase() && pair[1].is_ascii_uppercase());
  let pascal =
    characters.first().is_some_and(char::is_ascii_uppercase) && characters.iter().skip(1).any(char::is_ascii_uppercase) && characters.iter().any(char::is_ascii_lowercase);
  hump || pascal
}

fn path_like(text: &str) -> bool {
  text.contains('/') && !text.contains("://") && !text.contains(char::is_whitespace) && !text.starts_with('/')
}

fn code_mention(content: &str, line: u32) -> Option<RawMention> {
  let content = content.trim();
  if path_like(content) {
    return Some(RawMention {
      text: content.trim_start_matches("./").to_owned(),
      line,
      origin: Origin::Code,
      shape: Shape::Path,
    });
  }
  let called = content.contains('(') || content.contains('<');
  let mut name = content.split('(').next().unwrap_or(content).trim();
  name = name.trim_start_matches(['&', '*']).trim_end_matches(';');
  if let Some((head, _)) = name.split_once('<') {
    name = head;
  }
  if has_file_extension(name) || !is_name_like(name) {
    return None;
  }
  Some(RawMention {
    text: name.to_owned(),
    line,
    origin: Origin::Code,
    shape: Shape::Name {
      drift_eligible: called || name.contains('.') || name.contains("::") || is_camel(name),
    },
  })
}

fn fenced_mentions(text: &str, line: u32) -> Vec<RawMention> {
  let mut found: Vec<RawMention> = Vec::new();
  for token in text.split(|character: char| !(is_identifier_part(character) || character == '.' || character == ':')) {
    let token = token.trim_matches(['.', ':']);
    if !is_name_like(token) || FENCED_KEYWORDS.contains(&token) || found.iter().any(|known| known.text == token) {
      continue;
    }
    found.push(RawMention {
      text: token.to_owned(),
      line,
      origin: Origin::Fenced,
      shape: Shape::Name { drift_eligible: false },
    });
  }
  found
}

fn code_spans(text: &str) -> Vec<(Range<usize>, Range<usize>)> {
  let bytes = text.as_bytes();
  let mut spans = Vec::new();
  let mut index = 0;
  while index < bytes.len() {
    if bytes[index] != b'`' {
      index += 1;
      continue;
    }
    let run = bytes[index..].iter().take_while(|byte| **byte == b'`').count();
    let content_start = index + run;
    let mut search = content_start;
    let mut closed = None;
    while search < bytes.len() {
      if bytes[search] != b'`' {
        search += 1;
        continue;
      }
      let closing = bytes[search..].iter().take_while(|byte| **byte == b'`').count();
      if closing == run {
        closed = Some(search);
        break;
      }
      search += closing;
    }
    match closed {
      Some(end) => {
        spans.push((index..end + run, content_start..end));
        index = end + run;
      }
      None => index = content_start,
    }
  }
  spans
}

fn link_targets(text: &str) -> Vec<Range<usize>> {
  let mut targets = Vec::new();
  let mut from = 0;
  while let Some(offset) = text[from..].find("](") {
    let start = from + offset + 2;
    let Some(length) = text[start..].find(')') else { break };
    targets.push(start..start + length);
    from = start + length;
  }
  targets
}

fn link_mention(target: &str, line: u32) -> Option<RawMention> {
  let target = target.trim().trim_start_matches('<').trim_end_matches('>');
  let target = target.split_once(char::is_whitespace).map_or(target, |(path, _)| path);
  let path = target.split('#').next().unwrap_or(target);
  let local = !path.is_empty() && !path.contains(':') && !path.starts_with('/') && !path.contains(char::is_whitespace);
  local.then(|| RawMention {
    text: path.to_owned(),
    line,
    origin: Origin::Link,
    shape: Shape::Path,
  })
}

fn prose_mentions(text: &str, line: u32) -> Vec<RawMention> {
  let mut mentions = Vec::new();
  let mut blanked = text.to_owned().into_bytes();
  for (span, content) in code_spans(text) {
    mentions.extend(code_mention(&text[content], line));
    blanked[span].fill(b' ');
  }
  for target in link_targets(text) {
    mentions.extend(link_mention(&text[target.clone()], line));
    blanked[target].fill(b' ');
  }
  let prose = String::from_utf8_lossy(&blanked).into_owned();
  for word in prose.split_whitespace() {
    let word = word.trim_matches(|character: char| matches!(character, '(' | ')' | '[' | ']' | '<' | '>' | ',' | ';' | ':' | '"' | '\'' | '*' | '_' | '!' | '?'));
    let word = word.trim_end_matches('.');
    if path_like(word) && has_file_extension(word) {
      mentions.push(RawMention {
        text: word.trim_start_matches("./").to_owned(),
        line,
        origin: Origin::Prose,
        shape: Shape::Path,
      });
    }
  }
  mentions
}

pub(crate) fn line_ending(text: &str) -> &'static str {
  let crlf = text.matches("\r\n").count();
  match crlf * 2 > text.matches('\n').count() {
    true => "\r\n",
    false => "\n",
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Replacement {
  pub range: Range<usize>,
  pub text: String,
}

pub(crate) fn stamp_existing(text: &str, anchors: &[(&RawAnchor, String)]) -> Vec<Replacement> {
  let newline = line_ending(text);
  let mut edits: Vec<Replacement> = anchors
    .iter()
    .filter(|(anchor, hash)| anchor.body.as_deref() != Some(hash.as_str()))
    .map(|(anchor, hash)| match &anchor.body_value {
      Some(range) => Replacement {
        range: range.clone(),
        text: hash.clone(),
      },
      None => {
        let missing_newline = anchor.insert_at == text.len() && !text.ends_with('\n');
        let lead = if missing_newline { newline } else { "" };
        Replacement {
          range: anchor.insert_at..anchor.insert_at,
          text: format!("{lead}{}{BODY_KEY}: {hash}{newline}", " ".repeat(anchor.indent)),
        }
      }
    })
    .collect();
  edits.sort_by_key(|edit| edit.range.start);
  edits
}

pub(crate) fn new_frontmatter(text: &str, anchors: &[(String, String)]) -> Replacement {
  let newline = line_ending(text);
  let mut block = format!("{FRONTMATTER_DELIMITER}{newline}{ANCHORS_KEY}{newline}");
  for (target, hash) in anchors {
    block.push_str(&format!("  - {SYMBOL_KEY}: {target}{newline}    {BODY_KEY}: {hash}{newline}"));
  }
  block.push_str(&format!("{FRONTMATTER_DELIMITER}{newline}"));
  let at = bom_len(text);
  Replacement { range: at..at, text: block }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn apply(text: &str, edits: &[Replacement]) -> String {
    let mut out = text.to_owned();
    for edit in edits.iter().rev() {
      out.replace_range(edit.range.clone(), &edit.text);
    }
    out
  }

  fn names(section: &RawSection) -> Vec<(&str, u32, Origin, Shape)> {
    section
      .mentions
      .iter()
      .map(|mention| (mention.text.as_str(), mention.line, mention.origin, mention.shape))
      .collect()
  }

  #[test]
  fn should_split_at_atx_headings_outside_fences_and_ignore_setext_headings() {
    let text = "Intro line\n\n# Guide\nText\n```sh\n# not a heading\n```\n## Retries ##\nWait\nSetext title\n============\n### Deep\n#### \n#tag\n# Other\n";
    let doc = parse(text, "guide.md");
    let headings: Vec<(String, u32, u32)> = doc.sections.iter().map(|section| (section.heading_path(), section.start_line, section.end_line)).collect();
    assert_eq!(
      headings,
      [
        ("guide.md".to_owned(), 1, 2),
        ("Guide".to_owned(), 3, 7),
        ("Guide > Retries".to_owned(), 8, 11),
        ("Guide > Retries > Deep".to_owned(), 12, 12),
        ("Guide > Retries > Deep > ".to_owned(), 13, 14),
        ("Other".to_owned(), 15, 15),
      ]
    );
    assert_eq!(doc.sections[1].body, "Text\n```sh\n# not a heading\n```");
    assert_eq!(doc.sections[2].body, "Wait\nSetext title\n============");
  }

  #[test]
  fn should_skip_a_blank_preamble_and_name_a_headless_file_after_itself() {
    assert_eq!(parse("\n\n# Only\nx\n", "a.md").sections.iter().map(RawSection::heading_path).collect::<Vec<_>>(), ["Only"]);
    assert_eq!(parse("just text\n", "notes.md").sections[0].heading, ["notes.md"]);
  }

  #[test]
  fn should_extract_code_link_prose_and_fenced_mentions_with_their_shapes() {
    let text = "# Api\nCall `createClient()` or `ApiClient.request`, see [the client](../src/lib/client.ts#L3) and src/lib/retry.ts.\nAlso `crate::retry::Backoff`, `pageSize`, `package.json`, `a`, `the plan` and `https://x.io/a.ts`.\n```ts\nconst page = await listPages(limit);\nreturn client.fetchAll\n```\n";
    let doc = parse(text, "api.md");
    let name = |drift_eligible| Shape::Name { drift_eligible };
    assert_eq!(
      names(&doc.sections[0]),
      [
        ("createClient", 2, Origin::Code, name(true)),
        ("ApiClient.request", 2, Origin::Code, name(true)),
        ("../src/lib/client.ts", 2, Origin::Link, Shape::Path),
        ("src/lib/retry.ts", 2, Origin::Prose, Shape::Path),
        ("crate::retry::Backoff", 3, Origin::Code, name(true)),
        ("pageSize", 3, Origin::Code, name(true)),
        ("page", 5, Origin::Fenced, name(false)),
        ("listPages", 5, Origin::Fenced, name(false)),
        ("limit", 5, Origin::Fenced, name(false)),
        ("client.fetchAll", 6, Origin::Fenced, name(false)),
      ]
    );
  }

  #[test]
  fn should_mark_only_camel_case_dotted_pathed_or_called_names_as_drift_eligible() {
    let shape = |text: &str| code_mention(text, 1).map(|mention| mention.shape);
    assert_eq!(shape("retry"), Some(Shape::Name { drift_eligible: false }));
    assert_eq!(shape("Config"), Some(Shape::Name { drift_eligible: false }));
    assert_eq!(shape("retry()"), Some(Shape::Name { drift_eligible: true }));
    assert_eq!(shape("HttpClient"), Some(Shape::Name { drift_eligible: true }));
    assert_eq!(shape("Vec<Page>"), Some(Shape::Name { drift_eligible: true }));
    assert_eq!(shape("README.md"), None);
    assert_eq!(shape("two words"), None);
  }

  #[test]
  fn should_read_frontmatter_anchors_and_start_sections_after_it() {
    let text = "---\ntitle: Retries\nanchors:\n  - symbol: web:src/lib/api-client.ts#createClient   # the factory\n    body: 3f2a91c0d4e5\n  - symbol: \"pageSize\"\nowner: team\n---\n# Retries\nBody\n";
    let doc = parse(text, "retries.md");
    let anchors = doc.frontmatter.unwrap().anchors;
    assert_eq!(anchors.len(), 2);
    assert_eq!(
      (anchors[0].target.as_str(), anchors[0].body.as_deref(), anchors[0].line, anchors[0].indent),
      ("web:src/lib/api-client.ts#createClient", Some("3f2a91c0d4e5"), 4, 4)
    );
    assert_eq!(&text[anchors[0].body_value.clone().unwrap()], "3f2a91c0d4e5");
    assert_eq!(
      (anchors[1].target.as_str(), anchors[1].body.as_deref(), anchors[1].body_value.clone()),
      ("pageSize", None, None)
    );
    assert_eq!(
      doc.sections.iter().map(|section| (section.heading_path(), section.start_line)).collect::<Vec<_>>(),
      [("Retries".to_owned(), 9)]
    );
  }

  #[test]
  fn should_rewrite_only_body_values_and_keep_the_rest_of_the_file_byte_identical() {
    let text = "---\r\nanchors:\r\n  - symbol: web:src/a.ts#a   # keep\r\n    body: 000000000000  # old\r\n  - symbol: web:src/b.ts\r\n---\r\n# Note\r\n`a` and `b`\r\n";
    let doc = parse(text, "n.md");
    let anchors = doc.frontmatter.unwrap().anchors;
    let edits = stamp_existing(text, &[(&anchors[0], "111111111111".to_owned()), (&anchors[1], "222222222222".to_owned())]);
    assert_eq!(
      apply(text, &edits),
      "---\r\nanchors:\r\n  - symbol: web:src/a.ts#a   # keep\r\n    body: 111111111111  # old\r\n  - symbol: web:src/b.ts\r\n    body: 222222222222\r\n---\r\n# Note\r\n`a` and `b`\r\n"
    );
    assert_eq!(stamp_existing(text, &[(&anchors[0], "000000000000".to_owned())]), []);
  }

  #[test]
  fn should_prepend_frontmatter_after_a_bom_when_a_note_has_none() {
    let text = "\u{feff}# Note\nUse `createClient`.\n";
    let edit = new_frontmatter(text, &[("web:src/a.ts#createClient".to_owned(), "abcdefabcdef".to_owned())]);
    assert_eq!(
      apply(text, &[edit]),
      "\u{feff}---\nanchors:\n  - symbol: web:src/a.ts#createClient\n    body: abcdefabcdef\n---\n# Note\nUse `createClient`.\n"
    );
    let reparsed = parse(&apply(text, &[new_frontmatter(text, &[("x:y.ts#z".to_owned(), "1".repeat(12))])]), "n.md");
    assert_eq!(reparsed.frontmatter.unwrap().anchors[0].body.as_deref(), Some("111111111111"));
  }
}
