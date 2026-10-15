use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, Value};

const IDENTITY_KEYS: [&str; 2] = ["name", "path"];

pub(crate) fn merge_or_fresh(previous: &str, fresh: &str, unchanged: impl Fn(usize) -> bool, reproduces: impl Fn(&str) -> bool) -> String {
  let text = match merge_into(previous, fresh, unchanged) {
    Ok(merged) if reproduces(&merged) => merged,
    Ok(_) => {
      tracing::warn!("keeping comments would have changed the meaning of the file; wrote a fresh serialisation without them");
      fresh.to_owned()
    }
    Err(error) => {
      tracing::warn!(error = %atlas_detect::describe_toml_error(previous, error.message(), error.span()), "could not edit the file in place; wrote a fresh serialisation without its comments");
      fresh.to_owned()
    }
  };
  with_line_endings_of(previous, text)
}

pub(crate) fn with_line_endings_of(source: &str, text: String) -> String {
  match source.contains("\r\n") {
    true => text.replace("\r\n", "\n").replace('\n', "\r\n"),
    false => text,
  }
}

fn merge_into(previous: &str, fresh: &str, unchanged: impl Fn(usize) -> bool) -> Result<String, toml_edit::TomlError> {
  let mut document: DocumentMut = previous.parse()?;
  let fresh: DocumentMut = fresh.parse()?;
  for (key, fresh_item) in fresh.iter() {
    match (document.get_mut(key), fresh_item) {
      (Some(Item::ArrayOfTables(old)), Item::ArrayOfTables(new)) => merge_projects(old, new, &unchanged),
      (Some(old), new) => merge_item(old, new),
      (None, new) => {
        document.insert(key, detached(new));
      }
    }
  }
  let stale: Vec<String> = document.iter().map(|(key, _)| key.to_owned()).filter(|key| !fresh.contains_key(key)).collect();
  for key in stale {
    document.remove(&key);
  }
  Ok(document.to_string())
}

fn merge_projects(old: &mut ArrayOfTables, new: &ArrayOfTables, unchanged: &impl Fn(usize) -> bool) {
  let previous: Vec<Table> = old.iter().cloned().collect();
  let merged: Vec<Table> = new
    .iter()
    .map(|table| match matching(&previous, table) {
      Some(index) if unchanged(index) => previous[index].clone(),
      Some(index) => {
        let mut kept = previous[index].clone();
        merge_table(&mut kept, table);
        kept
      }
      None => detached_table(table),
    })
    .collect();
  old.clear();
  for table in merged {
    old.push(table);
  }
}

fn merge_item(old: &mut Item, new: &Item) {
  match (&mut *old, new) {
    (Item::Table(old_table), Item::Table(new_table)) => merge_table(old_table, new_table),
    (Item::ArrayOfTables(old_tables), Item::ArrayOfTables(new_tables)) => merge_tables(old_tables, new_tables),
    (Item::Value(old_value), Item::Value(new_value)) if same(old_value, new_value) => {}
    (Item::Value(old_value), Item::Value(new_value)) => {
      let decor = old_value.decor().clone();
      *old_value = new_value.clone();
      *old_value.decor_mut() = decor;
    }
    _ => *old = detached(new),
  }
}

fn merge_table(old: &mut Table, new: &Table) {
  let stale: Vec<String> = old.iter().map(|(key, _)| key.to_owned()).filter(|key| !new.contains_key(key)).collect();
  for key in stale {
    old.remove(&key);
  }
  for (key, new_item) in new.iter() {
    match old.get_mut(key) {
      Some(old_item) => merge_item(old_item, new_item),
      None => {
        old.insert(key, detached(new_item));
      }
    }
  }
}

fn merge_tables(old: &mut ArrayOfTables, new: &ArrayOfTables) {
  let previous: Vec<Table> = old.iter().cloned().collect();
  let merged: Vec<Table> = new
    .iter()
    .map(|table| match matching(&previous, table) {
      Some(index) => {
        let mut kept = previous[index].clone();
        merge_table(&mut kept, table);
        kept
      }
      None => detached_table(table),
    })
    .collect();
  old.clear();
  for table in merged {
    old.push(table);
  }
}

fn matching(previous: &[Table], table: &Table) -> Option<usize> {
  IDENTITY_KEYS.iter().find_map(|key| {
    let wanted = table.get(key).and_then(Item::as_str)?;
    previous.iter().position(|candidate| candidate.get(key).and_then(Item::as_str) == Some(wanted))
  })
}

fn same(left: &Value, right: &Value) -> bool {
  match (left, right) {
    (Value::String(left), Value::String(right)) => left.value() == right.value(),
    (Value::Integer(left), Value::Integer(right)) => left.value() == right.value(),
    (Value::Float(left), Value::Float(right)) => left.value() == right.value(),
    (Value::Boolean(left), Value::Boolean(right)) => left.value() == right.value(),
    (Value::Datetime(left), Value::Datetime(right)) => left.value() == right.value(),
    (Value::Array(left), Value::Array(right)) => left.len() == right.len() && left.iter().zip(right.iter()).all(|(left, right)| same(left, right)),
    (Value::InlineTable(left), Value::InlineTable(right)) => left.len() == right.len() && left.iter().all(|(key, value)| right.get(key).is_some_and(|other| same(value, other))),
    _ => false,
  }
}

fn detached(item: &Item) -> Item {
  match item {
    Item::Table(table) => Item::Table(detached_table(table)),
    Item::ArrayOfTables(tables) => {
      let mut detached_tables = ArrayOfTables::new();
      for table in tables.iter() {
        detached_tables.push(detached_table(table));
      }
      Item::ArrayOfTables(detached_tables)
    }
    other => other.clone(),
  }
}

fn detached_table(table: &Table) -> Table {
  let mut copy = table.clone();
  copy.set_position(None);
  for (_, item) in copy.iter_mut() {
    *item = detached(item);
  }
  copy
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn should_fall_back_to_the_fresh_text_when_the_merge_does_not_reproduce_the_intended_value() {
    let previous = "# comment\n[[project]]\nname = \"a\"\n";
    let fresh = "[[project]]\nname = \"a\"\nrepo = []\n";

    assert_eq!(merge_or_fresh(previous, fresh, |_| false, |_| false), fresh);
    assert_eq!(merge_or_fresh(previous, fresh, |_| false, |_| true), "# comment\n[[project]]\nname = \"a\"\nrepo = []\n");
  }

  #[test]
  fn should_keep_crlf_line_endings() {
    let previous = "# comment\r\n[[project]]\r\nname = \"a\"\r\n";
    let fresh = "[[project]]\nname = \"a\"\nrepo = []\n";

    assert_eq!(
      merge_or_fresh(previous, fresh, |_| false, |_| true),
      "# comment\r\n[[project]]\r\nname = \"a\"\r\nrepo = []\r\n"
    );
  }

  #[test]
  fn should_keep_comments_and_layout_while_changing_one_value() {
    let previous = "# registry\n[[project]]\nname = \"acme\" # main\nrepo = []\n\n[project.settings]\nmax_file_bytes = 1048576 # small\n";
    let fresh = "[[project]]\nname = \"acme\"\nrepo = []\n\n[project.settings]\nmax_file_bytes = 42\n";

    let merged = merge_into(previous, fresh, |_| false).unwrap();

    assert_eq!(
      merged,
      "# registry\n[[project]]\nname = \"acme\" # main\nrepo = []\n\n[project.settings]\nmax_file_bytes = 42 # small\n"
    );
  }

  #[test]
  fn should_keep_an_unchanged_project_verbatim_and_append_new_ones() {
    let previous = "[[project]]\nname = \"a\" # hand-written, sparse\n# end of file\n";
    let fresh = "[[project]]\nname = \"a\"\nrepo = []\n\n[[project]]\nname = \"b\"\nrepo = []\n";

    let merged = merge_into(previous, fresh, |index| index == 0).unwrap();

    assert_eq!(
      merged,
      "[[project]]\nname = \"a\" # hand-written, sparse\n\n[[project]]\nname = \"b\"\nrepo = []\n# end of file\n"
    );
  }

  #[test]
  fn should_follow_a_renamed_repo_by_its_path() {
    let previous = "[[project]]\nname = \"a\"\n\n[[project.repo]]\nname = \"old\" # renamed soon\npath = \"/src/x\"\n";
    let fresh = "[[project]]\nname = \"a\"\n\n[[project.repo]]\nname = \"new\"\npath = \"/src/x\"\n";

    let merged = merge_into(previous, fresh, |_| false).unwrap();

    assert_eq!(merged, "[[project]]\nname = \"a\"\n\n[[project.repo]]\nname = \"new\" # renamed soon\npath = \"/src/x\"\n");
  }
}
