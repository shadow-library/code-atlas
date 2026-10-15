use std::cmp::Ordering;

const PLACEHOLDER: &str = "{}";
const MAX_BASE_PATH_SEGMENTS: usize = 2;
const MIN_DYNAMIC_HEAD_TAIL_STATICS: u32 = 2;

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
  pub anchor: Vec<String>,
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
  pub dynamic_head_segments: u32,
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
    if self.dynamic_head_segments > 0 {
      confidence *= 0.6;
    }
    confidence
  }

  pub fn prefix_tier(&self) -> u8 {
    match (self.base_path_segments, self.dynamic_head_segments) {
      (0, 0) => 0,
      (_, 0) => 1,
      _ => 2,
    }
  }

  pub fn rule(&self) -> &'static str {
    if self.dynamic_on_static > 0 {
      "dynamic-on-static-segment"
    } else if self.literal_on_param > 0 {
      "literal-on-param-segment"
    } else if self.catch_all {
      "catch-all-route"
    } else if self.dynamic_head_segments > 0 {
      "dynamic-head-prefix"
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
  ClientPath {
    base,
    anchor: Vec::new(),
    segments,
  }
}

pub(crate) fn anchored_client_target(base: &str, target: &str) -> Option<ClientPath> {
  if !base.starts_with('/') || base.contains(PLACEHOLDER) || !target.starts_with(PLACEHOLDER) {
    return None;
  }
  let mut path = parse_client_target(target);
  path.anchor = base.split('/').filter(|segment| !segment.is_empty()).map(str::to_owned).collect();
  Some(path)
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
  let route = strip_anchor(&client.anchor, route)?;
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
  match best.filter(|found| found.statics > 0) {
    None if client.base == BaseKind::Dynamic && !client.anchor.is_empty() => dynamic_head(client, route),
    found => found,
  }
}

fn strip_anchor<'r>(anchor: &[String], route: &'r [RouteSegment]) -> Option<&'r [RouteSegment]> {
  let anchored = route.len() >= anchor.len()
    && anchor
      .iter()
      .zip(route)
      .all(|(expected, segment)| matches!(segment, RouteSegment::Static(value) if value == expected));
  anchored.then(|| &route[anchor.len()..])
}

fn dynamic_head(client: &ClientPath, route: &[RouteSegment]) -> Option<RouteMatch> {
  let fixed_length = route
    .iter()
    .all(|segment| matches!(segment, RouteSegment::Static(_) | RouteSegment::Param | RouteSegment::Pattern { .. }));
  let skips = match fixed_length {
    true => route
      .len()
      .checked_sub(client.segments.len())
      .filter(|skipped| *skipped > 0)
      .map_or(0..0, |skipped| skipped..skipped + 1),
    false => 1..route.len(),
  };
  let mut best = None;
  for skipped in skips {
    if route[..skipped].contains(&RouteSegment::CatchAll) {
      break;
    }
    let start = RouteMatch {
      dynamic_head_segments: skipped as u32,
      ..RouteMatch::default()
    };
    best = better(best, best_match(&client.segments, &route[skipped..], start));
  }
  best.filter(|found| found.statics >= MIN_DYNAMIC_HEAD_TAIL_STATICS)
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
  fn should_match_express_optional_markers_through_normalised_keys() {
    assert_eq!(normalize_route("/users/:id?"), "/users/{id?}");
    let without = matched("/users", "/users/:id?").unwrap();
    assert_eq!((without.statics, without.skipped_optional), (1, 1));
    assert_eq!(matched("/users/{}", "/users/:id?").unwrap().params, 1);
    assert_eq!(matched("/blog", "/blog/[[...slug]]").unwrap().skipped_optional, 1);
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

  fn anchored(target: &str, route: &str) -> Option<RouteMatch> {
    let client = anchored_client_target("/api", target).unwrap();
    match_route(&client, &parse_route(&normalize_route(route)))
  }

  #[test]
  fn should_let_an_anchored_dynamic_head_stand_for_any_leading_segments_before_a_static_tail_of_two() {
    let found = anchored("{}/versions/latest", "/api/projects/:id/drafts/:n/versions/latest").unwrap();
    assert_eq!((found.dynamic_head_segments, found.statics, found.params), (4, 2, 0));
    assert_eq!((found.rule(), found.prefix_tier()), ("dynamic-head-prefix", 2));
    assert!(found.confidence() < matched("{}/users/{}", "/api/users/:id").unwrap().confidence());
    let with_param = anchored("{}/versions/{}/restore", "/api/books/:id/versions/:v/restore").unwrap();
    assert_eq!((with_param.dynamic_head_segments, with_param.statics, with_param.params), (2, 2, 1));
    assert_eq!(anchored("{}/chapters", "/api/books/:id/chapters"), None);
    assert_eq!(anchored("{}/versions/{}", "/api/books/:id/versions/:v"), None);
  }

  #[test]
  fn should_never_let_an_unanchored_dynamic_head_skip_more_than_two_static_segments() {
    assert_eq!(matched("{}/oauth/token", "/partners/:partnerId/oauth/token"), None);
    assert_eq!(matched("{}/versions/latest", "/projects/:id/drafts/:n/versions/latest"), None);
    assert_eq!(matched("{}/oauth/token", "/v1/partners/oauth/token").map(|found| found.base_path_segments), Some(2));
    assert_eq!(matched("{}/oauth/token", "/a/b/c/oauth/token"), None);
  }

  #[test]
  fn should_keep_the_static_base_path_rule_ahead_of_the_dynamic_head_rule() {
    let prefixed = anchored("{}/versions/latest", "/api/v2/versions/latest").unwrap();
    assert_eq!((prefixed.base_path_segments, prefixed.dynamic_head_segments, prefixed.prefix_tier()), (1, 0, 1));
    assert_eq!(anchored("{}/versions/latest", "/api/versions/latest").unwrap().prefix_tier(), 0);
  }

  #[test]
  fn should_apply_the_dynamic_head_rule_to_dynamic_bases_only() {
    assert_eq!(matched("/x/versions/latest", "/projects/:id/versions/latest"), None);
    assert_eq!(matched("https://api.acme.test/versions/latest", "/projects/:id/versions/latest"), None);
  }

  #[test]
  fn should_try_every_head_length_on_routes_with_optional_or_catch_all_segments() {
    let optional = anchored("{}/versions/latest", "/api/projects/:id?/drafts/versions/latest").unwrap();
    assert_eq!((optional.dynamic_head_segments, optional.statics), (3, 2));
    let tail = anchored("{}/files/raw", "/api/repos/:id/files/raw/*").unwrap();
    assert_eq!((tail.dynamic_head_segments, tail.catch_all), (2, true));
    assert_eq!(anchored("{}/files/raw", "/api/*/:id/files/raw"), None);
  }

  #[test]
  fn should_anchor_a_dynamic_head_behind_a_known_static_base() {
    let anchored = anchored_client_target("/api/v1", "{}/versions/latest").unwrap();
    assert_eq!(
      (anchored.base.clone(), anchored.anchor.clone()),
      (BaseKind::Dynamic, vec!["api".to_owned(), "v1".to_owned()])
    );
    let found = match_route(&anchored, &parse_route("/api/v1/projects/{id}/versions/latest")).unwrap();
    assert_eq!((found.dynamic_head_segments, found.statics), (2, 2));
    assert_eq!(match_route(&anchored, &parse_route("/api/auth/projects/{id}/versions/latest")), None);
    assert_eq!(match_route(&anchored, &parse_route("/api")), None);
    assert_eq!(anchored_client_target("{}/api", "{}/versions/latest"), None);
    assert_eq!(anchored_client_target("https://api.acme.test", "{}/versions/latest"), None);
    assert_eq!(anchored_client_target("/api/v1", "versions/latest"), None);
  }

  #[test]
  fn should_treat_dynamic_route_pieces_as_params() {
    let found = matched("/tenants/{}/users", "/tenants/{}/users").unwrap();
    assert_eq!((found.statics, found.params), (2, 1));
  }
}
