mod common;

use atlas_detect::DecoratorFamily;
use atlas_project::ProjectSettings;
use atlas_query::{ContextAnswer, ContextItem, ContextRequest};

const FAMILY: &str = "packages = [\"@corvid/http\"]\n[controller]\ndecorators = [\"Controller\"]\n[routes.methods]\nGet = \"GET\"\nPost = \"POST\"\n";

const FILES: [(&str, &str); 13] = [
  (
    "package.json",
    "{ \"name\": \"corvid\", \"private\": true, \"workspaces\": [\"packages/*\", \"apps/*\"] }\n",
  ),
  (
    "packages/http/package.json",
    "{ \"name\": \"@corvid/http\", \"version\": \"1.0.0\", \"main\": \"src/index.ts\" }\n",
  ),
  (
    "packages/http/src/index.ts",
    "export * from './constants';\nexport * from './store';\nexport * from './route.decorator';\nexport * from './controller.decorator';\nexport * from './http-router';\nexport * from './encoding';\n",
  ),
  (
    "packages/http/src/constants.ts",
    "export const CONTROLLER_METADATA = 'corvid:controller';\nexport const ROUTE_METADATA = 'corvid:routes';\n",
  ),
  (
    "packages/http/src/store.ts",
    "const store = new WeakMap<object, Map<string, unknown>>();\n\nexport function defineMetadata(key: string, value: unknown, target: object): void {\n  const entries = store.get(target) ?? new Map<string, unknown>();\n  entries.set(key, value);\n  store.set(target, entries);\n}\n\nexport function getMetadata<T>(key: string, target: object): T | undefined {\n  return store.get(target)?.get(key) as T | undefined;\n}\n",
  ),
  (
    "packages/http/src/route.decorator.ts",
    "import { ROUTE_METADATA } from './constants';\nimport { defineMetadata, getMetadata } from './store';\n\nexport function Route(method: string, path = ''): MethodDecorator {\n  return (target, handler) => {\n    const routes = getMetadata<unknown[]>(ROUTE_METADATA, target.constructor) ?? [];\n    defineMetadata(ROUTE_METADATA, [...routes, { method, path, handler }], target.constructor);\n  };\n}\n\nexport function Get(path = ''): MethodDecorator {\n  return Route('GET', path);\n}\n\nexport function Post(path = ''): MethodDecorator {\n  return Route('POST', path);\n}\n",
  ),
  (
    "packages/http/src/controller.decorator.ts",
    "import { CONTROLLER_METADATA } from './constants';\nimport { defineMetadata } from './store';\n\nexport function Controller(prefix = ''): ClassDecorator {\n  return target => defineMetadata(CONTROLLER_METADATA, { prefix }, target);\n}\n",
  ),
  (
    "packages/http/src/encoding.ts",
    "import { gzipSync } from 'node:zlib';\n\nexport function pickCoding(accept: string, gzipPreferred = true): string {\n  return gzipPreferred && accept.includes('gzip') ? 'gzip' : 'plain';\n}\n\nexport function pack(body: Buffer, coding: string): Buffer {\n  return coding === 'gzip' ? gzipSync(body) : body;\n}\n",
  ),
  (
    "packages/http/src/http-router.ts",
    "import { CONTROLLER_METADATA, ROUTE_METADATA } from './constants';\nimport { pack, pickCoding } from './encoding';\nimport { getMetadata } from './store';\n\nexport class HttpRouter {\n  private readonly routes = new Map<string, () => Promise<Buffer>>();\n  register(controller: object): void {\n    const prefix = getMetadata<string>(CONTROLLER_METADATA, controller.constructor) ?? '';\n    for (const route of getMetadata<{ method: string; path: string }[]>(ROUTE_METADATA, controller.constructor) ?? []) {\n      this.routes.set(`${route.method} ${prefix}/${route.path}`, () => this.reply(controller));\n    }\n  }\n\n  private async reply(controller: object): Promise<Buffer> {\n    return this.send(Buffer.from(String(controller)), 'br, gzip');\n  }\n\n  private send(payload: Buffer, accept: string): Buffer {\n    return pack(payload, pickCoding(accept));\n  }\n}\n",
  ),
  (
    "apps/portal/package.json",
    "{ \"name\": \"corvid-portal\", \"private\": true, \"dependencies\": { \"@corvid/http\": \"workspace:*\" } }\n",
  ),
  (
    "apps/portal/src/feed.controller.ts",
    "import { Controller, Get, Post } from '@corvid/http';\n\n@Controller('feeds')\nexport class FeedController {\n  @Get('sitemap')\n  sitemap(): string {\n    return '<urlset/>';\n  }\n\n  @Post('ping')\n  ping(): string {\n    return 'ok';\n  }\n}\n",
  ),
  (
    "apps/portal/src/health.controller.ts",
    "import { Controller, Get } from '@corvid/http';\n\n@Controller('health')\nexport class HealthController {\n  @Get('live')\n  live(): string {\n    return 'ok';\n  }\n}\n",
  ),
  (
    "apps/portal/src/profile.controller.ts",
    "import { Controller, Post } from '@corvid/http';\n\n@Controller('profile')\nexport class ProfileController {\n  @Post('rename')\n  rename(): string {\n    return 'renamed';\n  }\n}\n",
  ),
];

const SHARED_HELPER_SCORE: f64 = 0.126459;
const PRIVATE_HELPER_SCORE: f64 = 0.227977;
const ROUTER: &str = "ts:corvid:@corvid/http:src/http-router::HttpRouter::register#method";

fn with_family() -> common::Fixture {
  let mut settings = ProjectSettings::default();
  settings.detect.route_families = vec!["corvid-http".to_owned()];
  settings.detect.families = vec![DecoratorFamily::parse("corvid-http", FAMILY).unwrap().family];
  common::single_repo_configured("corvid", &FILES, settings, common::engine_options())
}

fn answer(fixture: &common::Fixture, task: &str) -> ContextAnswer {
  fixture.query.context(&ContextRequest::new(task)).unwrap().answer
}

fn keys(items: &[ContextItem]) -> Vec<&str> {
  items.iter().map(|item| item.entity.key.as_str()).collect()
}

fn item<'a>(items: &'a [ContextItem], key: &str) -> Option<&'a ContextItem> {
  items.iter().find(|item| item.entity.key == key)
}

#[test]
fn should_reach_the_router_that_reads_the_route_decorator_metadata_from_a_named_endpoint() {
  let fixture = with_family();

  let answer = answer(&fixture, "GET /feeds/sitemap returns an empty body");

  let router = item(&answer.must_read, ROUTER).unwrap_or_else(|| panic!("{:?}", keys(&answer.must_read)));
  assert_eq!(
    router.why,
    "shares ROUTE_METADATA with the route decorator Get of FeedController.sitemap: references ROUTE_METADATA [resolved]"
  );
  assert!(!router.uncertain);
  let reply = "ts:corvid:@corvid/http:src/http-router::HttpRouter::reply#method";
  assert!(item(&answer.must_read, reply).or_else(|| item(&answer.skim, reply)).is_some(), "{:?}", keys(&answer.skim));
}

#[test]
fn should_take_neither_a_sibling_decorator_nor_the_handlers_of_other_routes_for_the_serving_path() {
  let fixture = with_family();

  let answer = answer(&fixture, "GET /feeds/sitemap returns an empty body");

  let sections = [&answer.must_read, &answer.skim, &answer.patterns, &answer.impact, &answer.uncertain];
  let shared: Vec<&str> = sections
    .iter()
    .flat_map(|items| items.iter())
    .filter(|item| item.why.starts_with("shares "))
    .map(|item| item.entity.key.as_str())
    .collect();
  assert_eq!(shared, [ROUTER]);
  let handlers: Vec<&str> = sections.iter().flat_map(|items| keys(items)).filter(|key| key.contains("HealthController")).collect();
  assert_eq!(handlers, Vec::<&str>::new());
  let patterns: Vec<(&str, &str)> = answer.patterns.iter().map(|item| (item.entity.key.as_str(), item.why.as_str())).collect();
  assert_eq!(
    patterns,
    [
      ("endpoint:corvid:GET /health/live", "pattern exemplar: GET on a route shaped like /feeds/sitemap"),
      ("endpoint:corvid:POST /feeds/ping", "same resource as /feeds/sitemap")
    ]
  );
}

#[test]
fn should_read_the_method_of_a_backticked_route() {
  let fixture = with_family();

  let answer = answer(&fixture, "`GET /feeds/sitemap` returns an empty body");

  let endpoint = item(&answer.skim, "endpoint:corvid:GET /feeds/sitemap").unwrap();
  assert_eq!(endpoint.why, "the endpoint GET /feeds/sitemap named in the task");
  assert!(item(&answer.must_read, ROUTER).is_some());
}

#[test]
fn should_keep_seeding_the_sibling_routes_when_the_named_route_does_not_exist() {
  let fixture = with_family();

  let answer = answer(&fixture, "add GET /feeds/archive to the feed controller");

  let endpoints: Vec<(&str, &str)> = answer
    .skim
    .iter()
    .filter(|item| item.entity.key.starts_with("endpoint:"))
    .map(|item| (item.entity.key.as_str(), item.why.as_str()))
    .collect();
  assert!(
    endpoints.contains(&("endpoint:corvid:GET /feeds/sitemap", "same resource as /feeds/archive")),
    "{endpoints:?}"
  );
  assert!(item(&answer.must_read, ROUTER).is_none());
}

#[test]
fn should_reach_the_caller_of_two_searched_primitives_without_endpoint_facts() {
  let fixture = common::single_repo("corvid", &FILES);

  let answer = answer(&fixture, "gzip output loses its bytes");

  let send = item(&answer.must_read, "ts:corvid:@corvid/http:src/http-router::HttpRouter::send#method").unwrap_or_else(|| panic!("{:?}", keys(&answer.must_read)));
  assert_eq!(send.why, "calls pickCoding [resolved]");
}

fn helper_scores(callers_path: &str) -> (f64, f64) {
  let callers: String = (0..12).map(|n| format!("export function caller{n}(): string {{\n  return audit('{n}');\n}}\n\n")).collect();
  let helpers = "export function audit(event: string): string {\n  return event;\n}\n\nexport function checksum(body: string): number {\n  return body.length;\n}\n";
  let seed = "import { audit, checksum } from './helpers';\n\nexport function signPayload(body: string): string {\n  audit('sign');\n  return String(checksum(body));\n}\n";
  let import = match callers_path.starts_with("test/") {
    true => "../src/helpers",
    false => "./helpers",
  };
  let fixture = common::single_repo(
    "web",
    &[
      ("package.json", "{ \"name\": \"web\" }"),
      ("src/helpers.ts", helpers),
      ("src/sign.ts", seed),
      (callers_path, &format!("import {{ audit }} from '{import}';\n\n{callers}")),
    ],
  );
  let mut request = ContextRequest::new("make `signPayload` stable");
  request.explain = true;

  let answer = fixture.query.context(&request).unwrap().answer;

  let scores = &answer.explain.as_ref().unwrap().scores;
  let score = |key: &str| scores.iter().find(|scored| scored.key == key).unwrap_or_else(|| panic!("{key} in {scores:?}")).score;
  (score("ts:web:web:src/helpers::audit#function"), score("ts:web:web:src/helpers::checksum#function"))
}

#[test]
fn should_rank_a_helper_shared_by_many_callers_below_one_only_the_seed_calls() {
  assert_eq!(helper_scores("src/callers.ts"), (SHARED_HELPER_SCORE, PRIVATE_HELPER_SCORE));
}

#[test]
fn should_not_discount_a_helper_whose_other_callers_are_tests() {
  assert_eq!(helper_scores("test/callers.test.ts"), (PRIVATE_HELPER_SCORE, PRIVATE_HELPER_SCORE));
}

#[test]
fn should_not_reach_a_caller_of_only_one_searched_primitive() {
  let encoding = "import { gzipSync } from 'node:zlib';\n\nexport function pickCoding(accept: string, gzipPreferred = true): string {\n  return gzipPreferred && accept.includes('gzip') ? 'gzip' : 'plain';\n}\n\nexport function pack(body: Buffer, coding: string): Buffer {\n  return coding === 'gzip' ? gzipSync(body) : body;\n}\n";
  let archive = "import { pack } from './encoding';\n\nexport function archive(payload: Buffer): Buffer {\n  return pack(payload, 'plain');\n}\n";
  let fixture = common::single_repo(
    "web",
    &[("package.json", "{ \"name\": \"web\" }"), ("src/encoding.ts", encoding), ("src/archive.ts", archive)],
  );

  let answer = answer(&fixture, "gzip output loses its bytes");

  let everything: Vec<&str> = [&answer.must_read, &answer.skim, &answer.impact, &answer.uncertain]
    .into_iter()
    .flat_map(|items| keys(items))
    .collect();
  assert_eq!(everything, ["ts:web:web:src/encoding::pickCoding#function", "ts:web:web:src/encoding::pack#function"]);
}

fn with_role_decorator() -> common::Fixture {
  let mut files: Vec<(&str, &str)> = FILES.iter().filter(|(path, _)| *path != "apps/portal/src/feed.controller.ts").copied().collect();
  files.extend([
    (
      "packages/http/src/role.decorator.ts",
      "import { defineMetadata } from './store';\n\nexport const ROLE_KEY = 'corvid:role';\n\nexport function RequireRole(role: string): MethodDecorator {\n  return (target, handler) => defineMetadata(ROLE_KEY, role, target);\n}\n",
    ),
    (
      "packages/http/src/role-guard.ts",
      "import { ROLE_KEY } from './role.decorator';\nimport { getMetadata } from './store';\n\nexport function allows(target: object, roles: string[]): boolean {\n  const wanted = getMetadata<string>(ROLE_KEY, target);\n  return !wanted || roles.includes(wanted);\n}\n",
    ),
    (
      "packages/http/src/route-types.ts",
      "export type HttpVerb = 'GET' | 'POST';\n\nexport interface RouteEntry {\n  method: HttpVerb;\n  path: string;\n}\n",
    ),
    (
      "apps/portal/src/route-table.ts",
      "import { type HttpVerb, type RouteEntry } from '@corvid/http';\n\nexport function describeRoute(route: RouteEntry): string {\n  return route.path;\n}\n\nexport function isSafeVerb(method: HttpVerb): boolean {\n  return method === 'GET';\n}\n",
    ),
    (
      "apps/portal/src/feed.controller.ts",
      "import { Controller, Get, Post, RequireRole } from '@corvid/http';\n\n@Controller('feeds')\nexport class FeedController {\n  @Get('sitemap')\n  @RequireRole('crawler')\n  sitemap(): string {\n    return '<urlset/>';\n  }\n\n  @Post('ping')\n  ping(): string {\n    return 'ok';\n  }\n}\n",
    ),
  ]);
  let index = "export * from './constants';\nexport * from './store';\nexport * from './route.decorator';\nexport * from './controller.decorator';\nexport * from './role.decorator';\nexport * from './route-types';\nexport * from './http-router';\nexport * from './encoding';\n";
  let files: Vec<(&str, &str)> = files
    .into_iter()
    .map(|(path, text)| (path, if path == "packages/http/src/index.ts" { index } else { text }))
    .collect();
  let route = "import { ROUTE_METADATA } from './constants';\nimport { type RouteEntry } from './route-types';\nimport { defineMetadata, getMetadata } from './store';\n\nexport function Route(method: RouteEntry['method'], path = ''): MethodDecorator {\n  return (target, handler) => {\n    const routes = getMetadata<RouteEntry[]>(ROUTE_METADATA, target.constructor) ?? [];\n    defineMetadata(ROUTE_METADATA, [...routes, { method, path, handler }], target.constructor);\n  };\n}\n\nexport function Get(path = ''): MethodDecorator {\n  return Route('GET', path);\n}\n\nexport function Post(path = ''): MethodDecorator {\n  return Route('POST', path);\n}\n";
  let files: Vec<(&str, &str)> = files
    .into_iter()
    .map(|(path, text)| (path, if path == "packages/http/src/route.decorator.ts" { route } else { text }))
    .collect();
  let mut settings = ProjectSettings::default();
  settings.detect.route_families = vec!["corvid-http".to_owned()];
  settings.detect.families = vec![DecoratorFamily::parse("corvid-http", FAMILY).unwrap().family];
  common::single_repo_configured("corvid", &files, settings, common::engine_options())
}

#[test]
fn should_follow_only_the_route_decorator_and_rank_value_keys_above_type_keys() {
  let fixture = with_role_decorator();

  let answer = answer(&fixture, "GET /feeds/sitemap returns an empty body");

  let sections = [&answer.must_read, &answer.skim, &answer.patterns, &answer.impact, &answer.uncertain];
  let mut shared: Vec<(&str, &str, f64)> = sections
    .iter()
    .flat_map(|items| items.iter())
    .filter(|item| item.why.starts_with("shares "))
    .map(|item| (item.entity.key.as_str(), item.why.as_str(), item.score))
    .collect();
  shared.sort_by(|left, right| right.2.total_cmp(&left.2));
  let shared: Vec<(&str, &str)> = shared.into_iter().map(|(key, why, _)| (key, why)).collect();
  assert_eq!(
    shared[0],
    (
      ROUTER,
      "shares ROUTE_METADATA with the route decorator Get of FeedController.sitemap: references ROUTE_METADATA [resolved]"
    )
  );
  assert!(shared.iter().all(|(_, why)| !why.contains("RequireRole") && !why.contains("ROLE_KEY")), "{shared:?}");
}
