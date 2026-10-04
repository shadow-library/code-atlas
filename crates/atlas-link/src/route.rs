use std::cmp::Ordering;

const PLACEHOLDER: &str = "{}";
const MAX_BASE_PATH_SEGMENTS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BaseKind {
  Relative,
  Dynamic,
  Absolute { host: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClientSegment {
  Literal(String),
  Dynamic,
  Pattern { prefix: String, suffix: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClientPath {
  pub base: BaseKind,
  pub segments: Vec<ClientSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RouteSegment {
  Static(String),
  Param,
  OptionalParam,
  CatchAll,
  Pattern { prefix: String, suffix: String },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RouteMatch {
  pub statics: u32,
  pub params: u32,
  pub literal_on_param: u32,
  pub dynamic_on_static: u32,
  pub partial_segments: u32,
  pub catch_all: bool,
  pub skipped_optional: u32,
  pub base_path_segments: u32,
}

impl RouteMatch {
  pub fn confidence(&self) -> f32 {
    let mut confidence = 0.62 + 0.1 * self.statics.min(2) as f32 + 0.05 * self.params.min(2) as f32;
    if self.literal_on_param > 0 {
      confidence *= 0.65;
    }
    if self.dynamic_on_static > 0 {
      confidence *= 0.5;
    }
    if self.partial_segments > 0 {
      confidence *= 0.8;
    }
    if self.catch_all {
      confidence *= 0.7;
    }
    if self.skipped_optional > 0 {
      confidence *= 0.95;
    }
    if self.base_path_segments > 0 {
      confidence *= 0.75;
    }
    confidence
  }

  pub fn rule(&self) -> &'static str {
    if self.dynamic_on_static > 0 {
      "dynamic-on-static-segment"
    } else if self.literal_on_param > 0 {
      "literal-on-param-segment"
    } else if self.catch_all {
      "catch-all-route"
    } else if self.base_path_segments > 0 {
      "base-path-prefix"
    } else if self.partial_segments > 0 {
      "partial-segment"
    } else {
      "path-template"
    }
  }

  pub fn rank(&self, other: &RouteMatch) -> Ordering {
    self
      .confidence()
      .total_cmp(&other.confidence())
      .then(self.statics.cmp(&other.statics))
      .then(self.params.cmp(&other.params))
  }

  fn add(mut self, step: Step) -> Self {
    match step {
      Step::Static => self.statics += 1,
      Step::Param => self.params += 1,
      Step::LiteralOnParam => self.literal_on_param += 1,
      Step::DynamicOnStatic => self.dynamic_on_static += 1,
      Step::Partial => self.partial_segments += 1,
    }
    self
  }
}

#[derive(Clone, Copy)]
enum Step {
  Static,
  Param,
  LiteralOnParam,
  DynamicOnStatic,
  Partial,
}

pub(crate) fn parse_client_target(target: &str) -> ClientPath {
  let path = target.split(['?', '#']).next().unwrap_or_default();
  let (base, rest) = split_base(path);
  let segments = rest.split('/').filter(|segment| !segment.is_empty()).map(client_segment).collect();
  ClientPath { base, segments }
}

fn split_base(path: &str) -> (BaseKind, &str) {
  let authority = path.strip_prefix("https://").or_else(|| path.strip_prefix("http://")).or_else(|| path.strip_prefix("//"));
  if let Some(rest) = authority {
    let host_end = rest.find('/').unwrap_or(rest.len());
    return (
      BaseKind::Absolute {
        host: rest[..host_end].to_ascii_lowercase(),
      },
      &rest[host_end..],
    );
  }
  match path.strip_prefix(PLACEHOLDER) {
    Some(rest) => (BaseKind::Dynamic, rest),
    None => (BaseKind::Relative, path),
  }
}

fn client_segment(segment: &str) -> ClientSegment {
  let (Some(first), Some(last)) = (segment.find(PLACEHOLDER), segment.rfind(PLACEHOLDER)) else {
    return ClientSegment::Literal(segment.to_owned());
  };
  let prefix = &segment[..first];
  let suffix = &segment[last + PLACEHOLDER.len()..];
  if prefix.is_empty() && suffix.is_empty() {
    return ClientSegment::Dynamic;
  }
  ClientSegment::Pattern {
    prefix: prefix.to_owned(),
    suffix: suffix.to_owned(),
  }
}

pub(crate) fn parse_route(route: &str) -> Vec<RouteSegment> {
  route.split('/').filter(|segment| !segment.is_empty()).map(route_segment).collect()
}

fn route_segment(segment: &str) -> RouteSegment {
  if matches!(segment, "*" | "(.*)" | "(.+)") {
    return RouteSegment::CatchAll;
  }
  if let Some(name) = segment.strip_prefix('{').and_then(|rest| rest.strip_suffix('}')) {
    return match name {
      _ if name.starts_with('*') || name.ends_with('*') || name.ends_with('+') => RouteSegment::CatchAll,
      _ if name.ends_with('?') => RouteSegment::OptionalParam,
      _ => RouteSegment::Param,
    };
  }
  match segment.split_once('*') {
    Some((prefix, suffix)) => RouteSegment::Pattern {
      prefix: prefix.to_owned(),
      suffix: suffix.to_owned(),
    },
    None => RouteSegment::Static(segment.to_owned()),
  }
}

pub(crate) fn match_route(client: &ClientPath, route: &[RouteSegment]) -> Option<RouteMatch> {
  let mut best = best_match(&client.segments, route, RouteMatch::default());
  if client.base != BaseKind::Relative {
    for skipped in 1..=MAX_BASE_PATH_SEGMENTS.min(route.len()) {
      if !route[..skipped].iter().all(|segment| matches!(segment, RouteSegment::Static(_))) {
        break;
      }
      let start = RouteMatch {
        base_path_segments: skipped as u32,
        ..RouteMatch::default()
      };
      best = better(best, best_match(&client.segments, &route[skipped..], start));
    }
  }
  best.filter(|found| found.statics > 0)
}

fn better(left: Option<RouteMatch>, right: Option<RouteMatch>) -> Option<RouteMatch> {
  match (left, right) {
    (Some(left), Some(right)) => Some(if right.rank(&left) == Ordering::Greater { right } else { left }),
    (left, right) => left.or(right),
  }
}

fn best_match(client: &[ClientSegment], route: &[RouteSegment], so_far: RouteMatch) -> Option<RouteMatch> {
  let Some((head, rest)) = route.split_first() else {
    return client.is_empty().then_some(so_far);
  };
  match head {
    RouteSegment::CatchAll => (0..=client.len()).fold(None, |best, taken| {
      let next = RouteMatch { catch_all: true, ..so_far };
      better(best, best_match(&client[taken..], rest, next))
    }),
    RouteSegment::OptionalParam => {
      let skipped = RouteMatch {
        skipped_optional: so_far.skipped_optional + 1,
        ..so_far
      };
      let consumed = client.split_first().and_then(|(first, others)| {
        let step = segment_step(first, &RouteSegment::Param)?;
        best_match(others, rest, so_far.add(step))
      });
      better(best_match(client, rest, skipped), consumed)
    }
    _ => {
      let (first, others) = client.split_first()?;
      let step = segment_step(first, head)?;
      best_match(others, rest, so_far.add(step))
    }
  }
}

fn segment_step(client: &ClientSegment, route: &RouteSegment) -> Option<Step> {
  match (client, route) {
    (ClientSegment::Literal(literal), RouteSegment::Static(value)) => (literal == value).then_some(Step::Static),
    (ClientSegment::Dynamic, RouteSegment::Static(_)) => Some(Step::DynamicOnStatic),
    (ClientSegment::Pattern { prefix, suffix }, RouteSegment::Static(value)) => fits(value, prefix, suffix).then_some(Step::Partial),
    (ClientSegment::Literal(_), RouteSegment::Param | RouteSegment::OptionalParam) => Some(Step::LiteralOnParam),
    (ClientSegment::Dynamic | ClientSegment::Pattern { .. }, RouteSegment::Param | RouteSegment::OptionalParam) => Some(Step::Param),
    (ClientSegment::Literal(literal), RouteSegment::Pattern { prefix, suffix }) => fits(literal, prefix, suffix).then_some(Step::LiteralOnParam),
    (ClientSegment::Dynamic | ClientSegment::Pattern { .. }, RouteSegment::Pattern { .. }) => Some(Step::Partial),
    (_, RouteSegment::CatchAll) => Some(Step::Param),
  }
}

fn fits(value: &str, prefix: &str, suffix: &str) -> bool {
  value.len() >= prefix.len() + suffix.len() && value.starts_with(prefix) && value.ends_with(suffix)
}

#[cfg(test)]
mod tests {
  use atlas_core::normalize_route;

  use super::*;

  fn matched(target: &str, route: &str) -> Option<RouteMatch> {
    match_route(&parse_client_target(target), &parse_route(&normalize_route(route)))
  }

  #[test]
  fn should_match_a_param_template_against_a_route_param() {
    let found = matched("{}/users/{}", "/users/:id").unwrap();
    assert_eq!((found.statics, found.params, found.literal_on_param), (1, 1, 0));
    assert_eq!(found.rule(), "path-template");
  }

  #[test]
  fn should_ignore_trailing_slashes_on_both_sides() {
    assert_eq!(matched("/api/items/", "/api/items").unwrap().statics, 2);
    assert_eq!(matched("/api/items", "/api/items/").unwrap().statics, 2);
  }

  #[test]
  fn should_drop_query_strings_and_fragments_before_matching() {
    let found = matched("/api/items?page={}&size=10#top", "/api/items").unwrap();
    assert_eq!((found.statics, found.params), (2, 0));
  }

  #[test]
  fn should_match_any_tail_with_a_wildcard_route_at_reduced_confidence() {
    let found = matched("/files/{}/raw/{}", "/files/*").unwrap();
    assert!(found.catch_all);
    assert_eq!(found.rule(), "catch-all-route");
    assert!(found.confidence() < matched("/files/{}", "/files/:name").unwrap().confidence());
    assert!(matched("/files/a/b", "/files/{*rest}").unwrap().catch_all);
    assert_eq!(matched("/other/a", "/files/*"), None);
  }

  #[test]
  fn should_match_optional_params_with_and_without_the_segment() {
    let optional = |target: &str| match_route(&parse_client_target(target), &parse_route("/users/{id?}"));
    let without = optional("/users").unwrap();
    assert_eq!((without.statics, without.skipped_optional), (1, 1));
    let with = optional("/users/{}").unwrap();
    assert_eq!((with.statics, with.params, with.skipped_optional), (1, 1, 0));
    assert_eq!(optional("/users/{}/posts"), None);
  }

  #[test]
  fn should_see_express_optional_markers_as_required_after_key_normalisation() {
    assert_eq!(normalize_route("/users/:id?"), "/users/{id}");
    assert_eq!(matched("/users", "/users/:id?"), None);
  }

  #[test]
  fn should_strip_a_dynamic_prefix_from_template_targets() {
    let client = parse_client_target("{}/orders/{}");
    assert_eq!(client.base, BaseKind::Dynamic);
    assert_eq!(client.segments, [ClientSegment::Literal("orders".into()), ClientSegment::Dynamic]);
    let joined = parse_client_target("{}orders");
    assert_eq!(joined.base, BaseKind::Dynamic);
    assert_eq!(joined.segments, [ClientSegment::Literal("orders".into())]);
  }

  #[test]
  fn should_allow_a_base_path_prefix_only_for_non_relative_bases() {
    let prefixed = matched("{}/users/{}", "/api/users/:id").unwrap();
    assert_eq!((prefixed.base_path_segments, prefixed.rule()), (1, "base-path-prefix"));
    assert_eq!(matched("/users/{}", "/api/users/:id"), None);
  }

  #[test]
  fn should_parse_absolute_hosts_out_of_the_target() {
    let client = parse_client_target("https://API.acme.dev:8443/orders/{}");
    assert_eq!(client.base, BaseKind::Absolute { host: "api.acme.dev:8443".into() });
    assert_eq!(client.segments.len(), 2);
  }

  #[test]
  fn should_rank_literal_on_param_below_template_matches() {
    let literal = matched("{}/users/me", "/users/:id").unwrap();
    assert_eq!(literal.rule(), "literal-on-param-segment");
    assert!(literal.confidence() < 0.5);
    assert!(literal.confidence() > 0.4);
  }

  #[test]
  fn should_reject_routes_of_different_arity_or_static_text() {
    assert_eq!(matched("/orders", "/orders/:id"), None);
    assert_eq!(matched("/api/status", "/api/items/:id"), None);
    assert_eq!(matched("{}/health", "/users/:id"), None);
  }

  #[test]
  fn should_require_at_least_one_static_segment() {
    assert_eq!(matched("{}/{}", "/:a"), None);
    assert_eq!(matched("{}/", "/"), None);
  }

  #[test]
  fn should_match_partial_segments_against_params() {
    let found = matched("/reports/report-{}.csv", "/reports/:file").unwrap();
    assert_eq!((found.statics, found.params), (1, 1));
    let partial = matched("/reports/{}.csv", "/reports/latest.csv").unwrap();
    assert_eq!(partial.partial_segments, 1);
  }

  #[test]
  fn should_treat_dynamic_route_pieces_as_params() {
    let found = matched("/tenants/{}/users", "/tenants/{}/users").unwrap();
    assert_eq!((found.statics, found.params), (2, 1));
  }
}
