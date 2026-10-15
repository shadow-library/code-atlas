use std::path::PathBuf;

use super::manifest::{Agent, Record, Scope};
use super::target::{Content, Current, Effect, Target};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Owner {
  pub(super) agent: Agent,
  pub(super) scope: Scope,
  pub(super) repo: Option<PathBuf>,
}

pub(super) struct Desired {
  pub(super) owner: Owner,
  pub(super) target: Target,
  pub(super) content: Content,
  pub(super) guard: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Flags {
  pub(super) force: bool,
  pub(super) migrate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Change {
  Unchanged,
  Created,
  Updated,
  Removed,
  Forgotten,
  Refused,
}

impl Change {
  pub(super) fn label(self) -> &'static str {
    match self {
      Self::Unchanged => "unchanged",
      Self::Created => "created",
      Self::Updated => "updated",
      Self::Removed => "removed",
      Self::Forgotten => "already gone, forgotten",
      Self::Refused => "refused",
    }
  }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Bookkeeping {
  Keep,
  Put(Record),
  Forget,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Redo {
  Write(Content),
  Remove { created_file: bool },
  AsPlanned,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Step {
  pub(super) owner: Owner,
  pub(super) target: Target,
  pub(super) change: Change,
  pub(super) effect: Option<Effect>,
  pub(super) redo: Redo,
  pub(super) confirm: Option<String>,
  pub(super) reason: Option<String>,
  pub(super) bookkeeping: Bookkeeping,
}

enum Gate {
  Drifted(&'static str),
  Legacy,
  Foreign,
  Guard(String),
  Unreadable(String),
}

impl Gate {
  fn passes(&self, flags: Flags) -> bool {
    match self {
      Self::Drifted(_) | Self::Foreign | Self::Guard(_) => flags.force,
      Self::Legacy | Self::Unreadable(_) => flags.migrate || flags.force,
    }
  }

  fn refusal(&self) -> String {
    match self {
      Self::Drifted(how) => format!("{how} since the last sync; pass --force to overwrite it"),
      Self::Legacy => "holds an entry from an older catl (an absolute command or a fixed --project); pass --migrate to replace it".to_owned(),
      Self::Foreign => "holds an entry that catl agents sync did not write; pass --force to replace it".to_owned(),
      Self::Guard(reason) => format!("{reason}; pass --force to write it anyway"),
      Self::Unreadable(reason) => format!("cannot be read ({reason}); pass --migrate to replace the entry"),
    }
  }

  fn confirmation(&self) -> String {
    match self {
      Self::Drifted(how) => format!("overwrite what {how} since the last sync"),
      Self::Legacy => "replace an entry written by an older catl".to_owned(),
      Self::Foreign => "replace an entry catl agents sync did not write".to_owned(),
      Self::Guard(reason) => reason.clone(),
      Self::Unreadable(reason) => format!("replace an entry that cannot be read ({reason})"),
    }
  }
}

impl Step {
  fn refused(owner: Owner, target: Target, reason: String) -> Self {
    Self {
      owner,
      target,
      change: Change::Refused,
      effect: None,
      redo: Redo::AsPlanned,
      confirm: None,
      reason: Some(reason),
      bookkeeping: Bookkeeping::Keep,
    }
  }
}

pub(super) fn decide_write(desired: Desired, recorded: Option<&Record>, flags: Flags) -> Step {
  let Desired { owner, target, content, guard } = desired;
  let hash = target.hash(&content);
  let current = target.current();
  let record = |created_file: bool| Record {
    agent: owner.agent,
    scope: owner.scope,
    repo: owner.repo.clone(),
    target: target.clone(),
    hash: hash.clone(),
    created_file,
  };
  let previously_created = recorded.is_some_and(|record| record.created_file);
  if matches!(&current, Current::Present { hash: present, .. } if *present == hash) {
    let bookkeeping = match recorded.is_some() || target.adopts_identical() {
      true => Bookkeeping::Put(record(previously_created)),
      false => Bookkeeping::Keep,
    };
    return Step {
      owner,
      target,
      change: Change::Unchanged,
      effect: None,
      redo: Redo::AsPlanned,
      confirm: None,
      reason: None,
      bookkeeping,
    };
  }
  let gate = match (&current, recorded) {
    (Current::Unreadable(reason), _) if matches!(target, Target::Registration { .. }) => Some(Gate::Unreadable(reason.clone())),
    (Current::Unreadable(reason), _) => {
      let reason = format!("cannot be read ({reason}); fix it by hand, Code Atlas will not overwrite it");
      return Step::refused(owner, target, reason);
    }
    (Current::Absent, Some(_)) => Some(Gate::Drifted("was removed")),
    (Current::Absent, None) => guard.map(Gate::Guard),
    (Current::Present { hash: present, .. }, Some(record)) if *present == record.hash => None,
    (Current::Present { .. }, Some(_)) => Some(Gate::Drifted("was changed")),
    (Current::Present { legacy: true, .. }, None) => Some(Gate::Legacy),
    (Current::Present { legacy: false, .. }, None) => Some(Gate::Foreign),
  };
  if let Some(gate) = gate.as_ref().filter(|gate| !gate.passes(flags)) {
    return Step::refused(owner, target, gate.refusal());
  }
  let effect = match target.write(&content) {
    Ok(effect) => effect,
    Err(reason) => return Step::refused(owner, target, reason),
  };
  let file_existed = target.file().exists();
  let confirm = gate
    .map(|gate| gate.confirmation())
    .or_else(|| (recorded.is_none() && file_existed && target.asks_before_touching_an_existing_file()).then(|| "change a file that already exists".to_owned()));
  let created_file = match &effect {
    Effect::Write { before: None, .. } => true,
    _ => previously_created,
  };
  Step {
    change: match current {
      Current::Absent => Change::Created,
      Current::Present { .. } | Current::Unreadable(_) => Change::Updated,
    },
    bookkeeping: Bookkeeping::Put(record(created_file)),
    owner,
    target,
    effect: Some(effect),
    redo: Redo::Write(content),
    confirm,
    reason: None,
  }
}

pub(super) fn decide_removal(record: &Record, flags: Flags) -> Step {
  let owner = Owner {
    agent: record.agent,
    scope: record.scope,
    repo: record.repo.clone(),
  };
  let target = record.target.clone();
  let forgotten = |owner: Owner, target: Target| Step {
    owner,
    target,
    change: Change::Forgotten,
    effect: None,
    redo: Redo::AsPlanned,
    confirm: None,
    reason: None,
    bookkeeping: Bookkeeping::Forget,
  };
  let confirm = match target.current() {
    Current::Absent => return forgotten(owner, target),
    Current::Present { hash, .. } if hash == record.hash => None,
    Current::Present { .. } if flags.force => Some("remove an entry that was changed since the last sync".to_owned()),
    Current::Present { .. } => return Step::refused(owner, target, "was changed since the last sync; pass --force to remove it anyway".to_owned()),
    Current::Unreadable(reason) if flags.force && matches!(target, Target::Registration { .. }) => Some(format!("remove an entry from a file that cannot be read ({reason})")),
    Current::Unreadable(reason) => return Step::refused(owner, target, format!("cannot be read ({reason}); fix it by hand")),
  };
  match target.remove(record.created_file) {
    Ok(Some(effect)) => Step {
      owner,
      target,
      change: Change::Removed,
      effect: Some(effect),
      redo: Redo::Remove {
        created_file: record.created_file,
      },
      confirm,
      reason: None,
      bookkeeping: Bookkeeping::Forget,
    },
    Ok(None) => forgotten(owner, target),
    Err(reason) => Step::refused(owner, target, reason),
  }
}

#[cfg(test)]
mod tests {
  use std::fs;

  use serde_json::json;

  use super::*;

  fn owner() -> Owner {
    Owner {
      agent: Agent::Claude,
      scope: Scope::Project,
      repo: Some(PathBuf::from("/r")),
    }
  }

  fn desired(target: &Target, command: &str) -> Desired {
    Desired {
      owner: owner(),
      target: target.clone(),
      content: Content::Server(json!({"type": "stdio", "command": command, "args": ["mcp", "--watch"]})),
      guard: None,
    }
  }

  fn applied(step: &Step) -> Record {
    let Some(Effect::Write { path, after, .. }) = &step.effect else {
      panic!("expected a write: {step:?}")
    };
    fs::write(path, after).unwrap();
    let Bookkeeping::Put(record) = &step.bookkeeping else { panic!("expected a record") };
    record.clone()
  }

  #[test]
  fn should_create_then_leave_alone_then_update_its_own_entry_without_asking() {
    let dir = tempfile::tempdir().unwrap();
    let target = Target::McpJson {
      path: dir.path().join(".mcp.json"),
    };

    let created = decide_write(desired(&target, "catl"), None, Flags::default());
    assert_eq!((created.change, created.confirm.clone()), (Change::Created, None));
    let record = applied(&created);
    assert!(record.created_file);
    let again = decide_write(desired(&target, "catl"), Some(&record), Flags::default());
    assert_eq!((again.change, again.effect.clone()), (Change::Unchanged, None));
    let updated = decide_write(desired(&target, "catl-next"), Some(&record), Flags::default());
    assert_eq!((updated.change, updated.confirm.clone()), (Change::Updated, None));
  }

  #[test]
  fn should_refuse_to_overwrite_or_remove_an_entry_the_user_changed_unless_forced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".mcp.json");
    let target = Target::McpJson { path: path.clone() };
    let record = applied(&decide_write(desired(&target, "catl"), None, Flags::default()));
    fs::write(&path, "{\"mcpServers\": {\"code-atlas\": {\"command\": \"catl\", \"args\": [\"mcp\"]}}}").unwrap();

    let refused = decide_write(desired(&target, "catl"), Some(&record), Flags::default());
    assert_eq!(refused.change, Change::Refused);
    assert_eq!(refused.reason.as_deref(), Some("was changed since the last sync; pass --force to overwrite it"));
    let forced = decide_write(desired(&target, "catl"), Some(&record), Flags { force: true, migrate: false });
    assert_eq!(forced.change, Change::Updated);
    assert_eq!(forced.confirm.as_deref(), Some("overwrite what was changed since the last sync"));
    assert_eq!(decide_removal(&record, Flags::default()).change, Change::Refused);
    assert_eq!(decide_removal(&record, Flags { force: true, migrate: false }).change, Change::Removed);
  }

  #[test]
  fn should_replace_a_legacy_entry_only_with_migrate_and_a_foreign_one_only_with_force() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".mcp.json");
    let target = Target::McpJson { path: path.clone() };
    fs::write(
      &path,
      "{\"mcpServers\": {\"code-atlas\": {\"command\": \"/bin/catl\", \"args\": [\"mcp\", \"--project\", \"acme\"]}}}",
    )
    .unwrap();
    assert_eq!(decide_write(desired(&target, "catl"), None, Flags::default()).change, Change::Refused);
    let migrated = decide_write(desired(&target, "catl"), None, Flags { force: false, migrate: true });
    assert_eq!(migrated.confirm.as_deref(), Some("replace an entry written by an older catl"));

    fs::write(&path, "{\"mcpServers\": {\"code-atlas\": {\"command\": \"catl\", \"args\": [\"serve\"]}}}").unwrap();
    assert_eq!(decide_write(desired(&target, "catl"), None, Flags { force: false, migrate: true }).change, Change::Refused);
    assert_eq!(decide_write(desired(&target, "catl"), None, Flags { force: true, migrate: false }).change, Change::Updated);
  }

  #[test]
  fn should_forget_a_record_whose_entry_is_already_gone_and_delete_a_file_it_created() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".mcp.json");
    let target = Target::McpJson { path: path.clone() };
    let record = applied(&decide_write(desired(&target, "catl"), None, Flags::default()));

    let removal = decide_removal(&record, Flags::default());
    assert!(matches!(removal.effect, Some(Effect::Delete { .. })), "{removal:?}");
    fs::remove_file(&path).unwrap();
    assert_eq!(decide_removal(&record, Flags::default()).change, Change::Forgotten);
  }
}
