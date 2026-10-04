use crate::model::AttrView;
use crate::queries::{ApiAnswer, LinkState, Related, SchemeSupport, SupportBasis, UsageState};
use crate::text::{RenderText, TextOut, caveats, entity_line, hop_text, listing};

fn related(item: &Related) -> String {
  format!("{} {}", item.entity.short, item.relation.certainty.tag)
}

fn redundant(attr: &AttrView, schemes_rendered: bool) -> bool {
  let empty = attr.value.as_array().is_some_and(Vec::is_empty);
  let repeated = schemes_rendered && matches!(attr.name.as_str(), "auth_requirement" | "auth_combinator");
  empty || repeated
}

fn attrs_text(attrs: &[AttrView]) -> String {
  attrs
    .iter()
    .map(|attr| format!("{}={} {}", attr.name, attr.value_text(), attr.tag()))
    .collect::<Vec<_>>()
    .join(", ")
}

fn joined(items: &[Related]) -> String {
  items.iter().map(related).collect::<Vec<_>>().join(", ")
}

impl RenderText for ApiAnswer {
  fn render_text(&self, out: &mut TextOut) {
    out.line(0, format!("endpoints ({})", self.endpoints.total));
    listing(out, 1, &self.endpoints, |out, indent, view| {
      out.line(
        indent,
        format!(
          "{} {} {} {}",
          view.method,
          view.route,
          view.endpoint.location.as_ref().map(ToString::to_string).unwrap_or_default(),
          view.certainty.tag
        ),
      );
      match &view.handler {
        Some(handler) => out.line(indent + 1, format!("handler {} {}", handler.entity.short, handler.relation.certainty.tag)),
        None => out.line(indent + 1, "handler unresolved"),
      }
      for guard in &view.guards {
        let schemes = match guard.schemes.is_empty() {
          true => String::new(),
          false => format!(", {}", guard.schemes.iter().map(|scheme| hop_text(&scheme.relation)).collect::<Vec<_>>().join(", ")),
        };
        out.line(indent + 1, format!("guard {}{schemes}", related(&guard.guard)));
      }
      for support in &view.auth {
        let combinator = support
          .combinator
          .as_ref()
          .map(|attr| format!(" ({} {})", attr.value_text(), attr.tag()))
          .unwrap_or_default();
        out.line(indent + 1, format!("{}{combinator}", hop_text(&support.scheme.relation)));
      }
      let extra: Vec<AttrView> = view.auth_attrs.iter().filter(|attr| !redundant(attr, !view.auth.is_empty())).cloned().collect();
      if !extra.is_empty() {
        out.line(indent + 1, format!("auth {}", attrs_text(&extra)));
      }
      if !view.permissions.is_empty() {
        out.line(indent + 1, format!("permissions {}", joined(&view.permissions)));
      }
      match (view.consumers.is_empty(), self.consumer_links) {
        (true, LinkState::Linked) => out.line(indent + 1, "consumers: none detected in project (external clients may exist)"),
        (true, LinkState::NotLinked) => out.line(indent + 1, "consumers: unknown (no client->endpoint links in index)"),
        (false, _) => {}
      }
      for consumer in &view.consumers {
        let via = consumer.client_call.as_ref().map(|call| format!(" via {}", call.short)).unwrap_or_default();
        out.line(
          indent + 1,
          format!("consumer {}{via}  {}", entity_line(&consumer.consumer.entity), hop_text(&consumer.consumer.relation)),
        );
        if !consumer.callers.is_empty() {
          out.line(indent + 2, format!("callers {}", joined(&consumer.callers)));
        }
        if !consumer.auth_used.is_empty() {
          out.line(indent + 2, format!("auth used {}", joined(&consumer.auth_used)));
        }
        let apps: Vec<&str> = consumer.applications.iter().map(|app| app.short.as_str()).collect();
        out.line(
          indent + 2,
          format!(
            "repos {} apps {}",
            consumer.repos.join(", "),
            if apps.is_empty() { "-".to_owned() } else { apps.join(", ") }
          ),
        );
      }
    });
    for matrix in &self.auth_matrix {
      out.line(0, format!("auth matrix {}", matrix.repo));
      for scheme in &matrix.schemes {
        let state = match scheme.used {
          UsageState::Used => format!("sent by clients in {}", scheme.used_by_repos.join(", ")),
          UsageState::Unused => "no detected client sends it".to_owned(),
          UsageState::Unknown => "client usage unknown (no uses_auth facts)".to_owned(),
        };
        let support = match &scheme.supported {
          Some(SchemeSupport {
            endpoints,
            basis: SupportBasis::SupportsAuth,
          }) => format!("supported by {endpoints} endpoints"),
          Some(SchemeSupport {
            endpoints,
            basis: SupportBasis::DerivedFromGuards,
          }) => format!("supported by {endpoints} endpoints (derived from guards, not linked)"),
          None => "endpoint support unknown".to_owned(),
        };
        let guards = match scheme.implemented_by.is_empty() {
          true => "-".to_owned(),
          false => scheme.implemented_by.iter().map(related).collect::<Vec<_>>().join(", "),
        };
        out.line(1, format!("{}: {state}; {support}; implemented by {guards}", scheme.scheme.short));
        for user in &scheme.used_by {
          out.line(2, format!("{} {}", entity_line(&user.entity), user.relation.certainty.tag));
        }
      }
    }
    if !self.unmatched_calls.is_empty() {
      out.line(0, format!("unmatched client call sites ({})", self.unmatched_calls.total));
      listing(out, 1, &self.unmatched_calls, |out, indent, call| {
        let site = call.site.as_ref().map(|site| format!(" at {site}")).unwrap_or_default();
        let detail = call
          .unmatched_reason
          .as_ref()
          .map(|attr| format!(" ({} {})", attr.value_text(), attr.tag()))
          .unwrap_or_default();
        out.line(indent, format!("{} from {}{site}: {}{detail}", call.call.short, call.caller.entity.short, call.reason));
      });
    }
    caveats(out, &self.caveats);
  }
}
