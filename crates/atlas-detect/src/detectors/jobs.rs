use atlas_core::{ArgSummary, EdgeKind, EntityKey, EntityKind, FileFacts, LanguageFamily, NodeRef, RefKind, ReferenceFact};
use serde_json::json;

use super::detector;
use crate::cert::Cert;
use crate::sink::{Sink, attrs};
use crate::view::FileView;

pub struct JobDetector;

detector!(JobDetector, "jobs", 1);

const NEST_SCHEDULE: [&str; 1] = ["@nestjs/schedule"];
const SCHEDULE_DECORATORS: [&str; 3] = ["Cron", "Interval", "Timeout"];
const CRON_PACKAGES: [&str; 2] = ["node-cron", "cron"];

impl JobDetector {
  fn prefilter(&self, facts: &FileFacts) -> bool {
    facts.references.iter().any(|reference| {
      reference
        .path
        .last()
        .is_some_and(|name| SCHEDULE_DECORATORS.contains(&name.as_str()) || matches!(name.as_str(), "schedule" | "setInterval" | "new" | "new_async"))
    })
  }

  fn run(&self, view: &FileView<'_>, sink: &mut Sink) {
    match view.family() {
      LanguageFamily::Rust => self.rust_jobs(view, sink),
      _ => {
        self.decorated_jobs(view, sink);
        self.scheduled_calls(view, sink);
      }
    }
  }

  fn decorated_jobs(&self, view: &FileView<'_>, sink: &mut Sink) {
    for symbol in view.facts.symbols.iter().filter(|symbol| symbol.kind == EntityKind::Method) {
      for attribute in &symbol.attributes {
        let Some(trigger) = SCHEDULE_DECORATORS
          .iter()
          .find(|name| matches!(attribute.path.as_slice(), [local] if view.imports_named(local, &NEST_SCHEDULE, name)))
        else {
          continue;
        };
        let schedule = schedule_text(attribute.args.first());
        let job = job_key(view, &view.qualified_name(symbol));
        let job_attrs = attrs([
          ("experimental", json!(true)),
          ("trigger", json!(trigger.to_ascii_lowercase())),
          ("schedule", schedule.map_or(json!(null), |schedule| json!(schedule))),
          ("handler", json!(view.qualified_name(symbol))),
        ]);
        let cert = Cert::heuristic(0.85, "nest-schedule-decorator");
        sink.entity(job.clone(), attribute.span, cert, job_attrs.clone());
        sink.edge(EdgeKind::Defines, NodeRef::Local(symbol.local), NodeRef::Key(job), attribute.span, cert, job_attrs);
      }
    }
  }

  fn scheduled_calls(&self, view: &FileView<'_>, sink: &mut Sink) {
    for reference in &view.facts.references {
      let (trigger, schedule, handler) = match (reference.kind, reference.path.as_slice()) {
        (RefKind::MethodCall, [receiver, method]) if method == "schedule" && view.imports_module(receiver, &CRON_PACKAGES) => {
          ("cron", schedule_text(reference.args.first()), reference.args.get(1))
        }
        (RefKind::Call, [name]) if name == "setInterval" && reference.from.is_none() && view.ts_import(name).is_none() => {
          ("interval", schedule_text(reference.args.get(1)), reference.args.first())
        }
        _ => continue,
      };
      let Some(schedule) = schedule else { continue };
      emit_call_job(view, sink, reference, trigger, &schedule, handler);
    }
  }

  fn rust_jobs(&self, view: &FileView<'_>, sink: &mut Sink) {
    for reference in view.facts.references.iter().filter(|reference| reference.kind == RefKind::Call) {
      let Some(last) = reference.path.last() else { continue };
      if !matches!(last.as_str(), "new" | "new_async") {
        continue;
      }
      let job_type = reference.path.len().checked_sub(2).and_then(|index| reference.path.get(index));
      let is_cron_job = job_type.is_some_and(|name| name == "Job")
        && (view.rust_path_is(&reference.path, &["tokio_cron_scheduler", "Job", last.as_str()]) || reference.path.first().is_some_and(|first| first == "tokio_cron_scheduler"));
      if !is_cron_job {
        continue;
      }
      let Some(schedule) = schedule_text(reference.args.first()) else { continue };
      emit_call_job(view, sink, reference, "cron", &schedule, reference.args.get(1));
    }
  }
}

fn emit_call_job(view: &FileView<'_>, sink: &mut Sink, reference: &ReferenceFact, trigger: &str, schedule: &str, handler: Option<&ArgSummary>) {
  let handler_name = match handler {
    Some(ArgSummary::Path(path)) => Some(path.join(".")),
    _ => None,
  };
  let name = handler_name.clone().unwrap_or_else(|| format!("{trigger} {schedule}"));
  let job = job_key(view, &name);
  let job_attrs = attrs([
    ("experimental", json!(true)),
    ("trigger", json!(trigger)),
    ("schedule", json!(schedule)),
    ("handler", handler_name.map_or(json!(null), |handler| json!(handler))),
  ]);
  let cert = Cert::heuristic(0.7, "scheduler-call");
  sink.entity(job.clone(), reference.span, cert, job_attrs.clone());
  sink.edge(EdgeKind::Defines, view.node(reference.from), NodeRef::Key(job), reference.span, cert, job_attrs);
}

fn job_key(view: &FileView<'_>, name: &str) -> EntityKey {
  EntityKey::Job {
    repo: view.repo().clone(),
    name: name.to_owned(),
  }
}

fn schedule_text(arg: Option<&ArgSummary>) -> Option<String> {
  match arg? {
    ArgSummary::Str(value) => Some(value.clone()),
    ArgSummary::Number(value) => Some(value.clone()),
    ArgSummary::Path(path) => Some(path.join(".")),
    _ => None,
  }
}
