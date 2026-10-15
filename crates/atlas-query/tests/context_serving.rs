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
fn should_follow_every_decorator_and_rank_the_route_decorators_consumer_first() {
  let fixture = with_role_decorator();

  let answer = explained(&fixture, "GET /feeds/sitemap returns an empty body");

  let shared: Vec<(&str, &str)> = shared_by_score(&answer).into_iter().map(|(key, why, _)| (key, why)).collect();
  assert_eq!(
    shared,
    [
      (
        ROUTER,
        "shares ROUTE_METADATA with the route decorator Get of FeedController.sitemap: references ROUTE_METADATA [resolved]"
      ),
      (
        "ts:corvid:@corvid/http:src/role-guard::allows#function",
        "shares ROLE_KEY with the decorator RequireRole of FeedController.sitemap: references ROLE_KEY [resolved]"
      ),
    ]
  );
  let handler = "ts:corvid:corvid-portal:src/feed.controller::FeedController::sitemap#method";
  let weights = [ROUTER, "ts:corvid:@corvid/http:src/role-guard::allows#function"].map(|to| expansion_weight(&answer, handler, to));
  assert_eq!(weights, [0.63, 0.315]);
}

fn shared_by_score(answer: &ContextAnswer) -> Vec<(&str, &str, f64)> {
  let sections = [&answer.must_read, &answer.skim, &answer.patterns, &answer.impact, &answer.uncertain];
  let mut shared: Vec<(&str, &str, f64)> = sections
    .iter()
    .flat_map(|items| items.iter())
    .filter(|item| item.why.starts_with("shares "))
    .map(|item| (item.entity.key.as_str(), item.why.as_str(), item.score))
    .collect();
  shared.sort_by(|left, right| right.2.total_cmp(&left.2).then_with(|| left.0.cmp(right.0)));
  shared
}

const WEB_FAMILY: &str = "packages = [\"@corvid/web\"]\n[controller]\ndecorators = [\"Resource\"]\n[routes.methods]\nFetch = \"GET\"\nStore = \"POST\"\nReplace = \"PUT\"\nAmend = \"PATCH\"\nRemove = \"DELETE\"\nProbe = \"HEAD\"\nPreflight = \"OPTIONS\"\nEvery = \"ANY\"\n";

const WEB_FILES: [(&str, &str); 25] = [
  (
    "package.json",
    "{ \"name\": \"corvid\", \"private\": true, \"workspaces\": [\"packages/*\", \"apps/*\"] }\n",
  ),
  (
    "packages/meta/package.json",
    "{ \"name\": \"@corvid/meta\", \"version\": \"2.1.0\", \"main\": \"src/index.ts\" }\n",
  ),
  ("packages/meta/src/index.ts", "export * from './meta-store';\n"),
  (
    "packages/meta/src/meta-store.ts",
    "const bags = new WeakMap<object, Map<string | symbol, unknown>>();\n\nexport class MetaStore {\n  static merge<T>(key: string | symbol, value: T, target: object): void {\n    const bag = bags.get(target) ?? new Map<string | symbol, unknown>();\n    bag.set(key, value);\n    bags.set(target, bag);\n  }\n\n  static read<T>(key: string | symbol, target: object): T | undefined {\n    return bags.get(target)?.get(key) as T | undefined;\n  }\n\n  static has(key: string | symbol, target: object): boolean {\n    return bags.get(target)?.has(key) ?? false;\n  }\n}\n",
  ),
  (
    "packages/core/package.json",
    "{ \"name\": \"@corvid/core\", \"version\": \"2.1.0\", \"main\": \"src/index.ts\", \"dependencies\": { \"@corvid/meta\": \"workspace:*\" } }\n",
  ),
  (
    "packages/core/src/index.ts",
    "export * from './entry';\nexport * from './transport';\nexport * from './scanner';\nexport * from './kernel';\n",
  ),
  (
    "packages/core/src/entry.ts",
    "import { MetaStore } from '@corvid/meta';\n\nexport const ENTRY_KEY = Symbol('corvid:entry');\n\nexport function markEntry(options: object): MethodDecorator {\n  return (_target, _property, descriptor) => MetaStore.merge(ENTRY_KEY, options, descriptor.value as object);\n}\n",
  ),
  (
    "packages/core/src/transport.ts",
    "export interface EntryBundle {\n  unit: object;\n  method: (...args: unknown[]) => unknown;\n  options: object;\n}\n\nexport abstract class Transport {\n  abstract mount(bundles: EntryBundle[]): void;\n}\n",
  ),
  (
    "packages/core/src/scanner.ts",
    "import { MetaStore } from '@corvid/meta';\n\nimport { ENTRY_KEY } from './entry';\nimport { type Kernel } from './kernel';\nimport { type EntryBundle } from './transport';\n\nexport class Scanner {\n  constructor(private readonly kernel: Kernel) {}\n\n  collect(unit: object): void {\n    const prototype = Object.getPrototypeOf(unit) as Record<string, (...args: unknown[]) => unknown>;\n    const bundles: EntryBundle[] = [];\n    for (const property of Object.getOwnPropertyNames(prototype)) {\n      const options = MetaStore.read<object>(ENTRY_KEY, prototype[property]);\n      if (options) bundles.push({ unit, method: prototype[property], options });\n    }\n    const transport = this.kernel.transport;\n    transport.mount(bundles);\n  }\n}\n",
  ),
  (
    "packages/core/src/kernel.ts",
    "import { Scanner } from './scanner';\nimport { type Transport } from './transport';\n\nexport class Kernel {\n  private readonly scanner = new Scanner(this);\n\n  constructor(private readonly carrier: Transport) {}\n\n  get transport(): Transport {\n    return this.carrier;\n  }\n\n  load(unit: object): void {\n    this.scanner.collect(unit);\n  }\n}\n",
  ),
  (
    "packages/web/package.json",
    "{ \"name\": \"@corvid/web\", \"version\": \"2.1.0\", \"main\": \"src/index.ts\", \"dependencies\": { \"@corvid/core\": \"workspace:*\", \"@corvid/meta\": \"workspace:*\" } }\n",
  ),
  (
    "packages/web/src/index.ts",
    "export * from './verb';\nexport * from './route.options';\nexport * from './route.decorators';\nexport * from './resource.decorator';\nexport * from './params';\nexport * from './reply';\nexport * from './web-router';\n",
  ),
  (
    "packages/web/src/verb.ts",
    "export enum Verb {\n  Fetch = 'GET',\n  Store = 'POST',\n  Replace = 'PUT',\n  Amend = 'PATCH',\n  Remove = 'DELETE',\n  Probe = 'HEAD',\n  Preflight = 'OPTIONS',\n  Every = 'ALL',\n}\n",
  ),
  (
    "packages/web/src/route.options.ts",
    "import { type Verb } from './verb';\n\nexport interface RouteOptions {\n  verb: Verb;\n  path?: string;\n}\n",
  ),
  (
    "packages/web/src/route.decorators.ts",
    "import { markEntry } from '@corvid/core';\n\nimport { type RouteOptions } from './route.options';\nimport { Verb } from './verb';\n\nexport function defineRoute(options: RouteOptions): MethodDecorator {\n  return markEntry(options);\n}\n\nexport const Fetch = (path?: string) => defineRoute({ verb: Verb.Fetch, path });\nexport const Store = (path?: string) => defineRoute({ verb: Verb.Store, path });\nexport const Replace = (path?: string) => defineRoute({ verb: Verb.Replace, path });\nexport const Amend = (path?: string) => defineRoute({ verb: Verb.Amend, path });\nexport const Remove = (path?: string) => defineRoute({ verb: Verb.Remove, path });\nexport const Probe = (path?: string) => defineRoute({ verb: Verb.Probe, path });\nexport const Preflight = (path?: string) => defineRoute({ verb: Verb.Preflight, path });\nexport const Every = (path?: string) => defineRoute({ verb: Verb.Every, path });\n",
  ),
  (
    "packages/web/src/resource.decorator.ts",
    "import { MetaStore } from '@corvid/meta';\n\nexport const RESOURCE_KIND = Symbol('corvid:web:resource');\n\nexport function Resource(options: { order?: number } = {}): ClassDecorator {\n  return target => MetaStore.merge(RESOURCE_KIND, { order: options.order ?? 0 }, target);\n}\n",
  ),
  (
    "packages/web/src/params.ts",
    "import { MetaStore } from '@corvid/meta';\n\nexport const INPUT_SLOTS = Symbol('corvid:web:inputs');\n\nexport function bindInput(kind: string): ParameterDecorator {\n  return (target, property, index) => MetaStore.merge(INPUT_SLOTS, [{ index, kind }], (target as Record<string | symbol, object>)[property as string]);\n}\n\nexport function ReplyParam(): ParameterDecorator {\n  return bindInput('reply');\n}\n\nexport const Reply = ReplyParam;\nexport const Payload = () => bindInput('payload');\n",
  ),
  (
    "packages/web/src/reply.ts",
    "export class ReplyHandle {\n  content: string | undefined;\n\n  get sent(): boolean {\n    return this.content !== undefined;\n  }\n\n  send(content: string): void {\n    this.content = content;\n  }\n}\n",
  ),
  (
    "packages/web/src/web-router.ts",
    "import { type EntryBundle, Transport } from '@corvid/core';\nimport { MetaStore } from '@corvid/meta';\n\nimport { INPUT_SLOTS } from './params';\nimport { ReplyHandle } from './reply';\nimport { RESOURCE_KIND } from './resource.decorator';\n\nexport class WebRouter extends Transport {\n  private readonly routes = new Map<string, () => Promise<string>>();\n\n  mount(bundles: EntryBundle[]): void {\n    for (const bundle of this.sortResources(bundles)) {\n      const { verb, path } = bundle.options as { verb: string; path?: string };\n      this.routes.set(`${verb} ${path ?? '/'}`, this.buildInvoker(bundle));\n    }\n  }\n\n  private sortResources(bundles: EntryBundle[]): EntryBundle[] {\n    return bundles.filter(bundle => MetaStore.has(RESOURCE_KIND, bundle.unit.constructor));\n  }\n\n  private buildInvoker(bundle: EntryBundle): () => Promise<string> {\n    const slots = MetaStore.read<{ index: number }[]>(INPUT_SLOTS, bundle.method) ?? [];\n    return async () => {\n      const reply = new ReplyHandle();\n      const pending = bundle.method.apply(bundle.unit, slots.map(() => reply));\n      if (reply.sent) return reply.content ?? '';\n      return String(await pending);\n    };\n  }\n}\n",
  ),
  (
    "apps/portal/package.json",
    "{ \"name\": \"corvid-portal\", \"private\": true, \"dependencies\": { \"@corvid/core\": \"workspace:*\", \"@corvid/web\": \"workspace:*\" } }\n",
  ),
  (
    "apps/portal/src/feed.controller.ts",
    "import { Fetch, Reply, type ReplyHandle, Resource } from '@corvid/web';\n\n@Resource()\nexport class FeedController {\n  @Fetch('/feeds/export')\n  async feedExport(@Reply() reply: ReplyHandle) {\n    reply.send(await Promise.resolve('<feed/>'));\n  }\n}\n",
  ),
  (
    "apps/portal/src/health.controller.ts",
    "import { Fetch, Payload, Reply, type ReplyHandle, Resource, Store } from '@corvid/web';\n\n@Resource({ order: -1 })\nexport class HealthController {\n  @Fetch('/health/live')\n  live(@Reply() reply: ReplyHandle) {\n    reply.send('ok');\n  }\n\n  @Store('/health/echo')\n  echo(@Payload() payload: string) {\n    return payload;\n  }\n}\n",
  ),
  (
    "apps/portal/src/legacy.controller.ts",
    "import { defineRoute, Reply, type ReplyHandle, Resource, Verb } from '@corvid/web';\n\n@Resource({ order: 10 })\nexport class LegacyController {\n  @defineRoute({ verb: Verb.Every, path: '/legacy/feed.rss' })\n  rss(@Reply() reply: ReplyHandle) {\n    reply.send('<rss/>');\n  }\n}\n",
  ),
  (
    "apps/portal/src/main.ts",
    "import { Kernel } from '@corvid/core';\nimport { WebRouter } from '@corvid/web';\n\nimport { FeedController } from './feed.controller';\nimport { HealthController } from './health.controller';\nimport { LegacyController } from './legacy.controller';\n\nexport const kernel = new Kernel(new WebRouter());\nfor (const unit of [new HealthController(), new FeedController(), new LegacyController()]) kernel.load(unit);\n",
  ),
  ("apps/portal/src/route-alias.ts", "export const ROUTE_ALIASES = ['/feeds/export'];\n"),
];

const EXPORT_TASK: &str = "`GET /feeds/export` answers 204 with nothing in it instead of the Atom feed";
const INVOKER: &str = "ts:corvid:@corvid/web:src/web-router::WebRouter::buildInvoker#method";
const SORTER: &str = "ts:corvid:@corvid/web:src/web-router::WebRouter::sortResources#method";
const HANDLER: &str = "ts:corvid:corvid-portal:src/feed.controller::FeedController::feedExport#method";

fn web_fixture(replaced: &[(&str, &str)]) -> common::Fixture {
  let mut files: Vec<(&str, &str)> = WEB_FILES
    .iter()
    .map(|(path, text)| (*path, replaced.iter().find(|(other, _)| other == path).map_or(*text, |(_, text)| *text)))
    .collect();
  files.extend(replaced.iter().filter(|(path, _)| WEB_FILES.iter().all(|(other, _)| other != path)));
  let mut settings = ProjectSettings::default();
  settings.detect.route_families = vec!["corvid-web".to_owned()];
  settings.detect.families = vec![DecoratorFamily::parse("corvid-web", WEB_FAMILY).unwrap().family];
  common::single_repo_configured("corvid", &files, settings, common::engine_options())
}

fn explained(fixture: &common::Fixture, task: &str) -> ContextAnswer {
  let mut request = ContextRequest::new(task);
  request.explain = true;
  fixture.query.context(&request).unwrap().answer
}

fn expansion_weight(answer: &ContextAnswer, from: &str, to: &str) -> f64 {
  let prefix = format!("{from} -> {to} (");
  let line = answer
    .explain
    .as_ref()
    .unwrap()
    .expansions
    .iter()
    .find(|line| line.starts_with(&prefix))
    .unwrap_or_else(|| panic!("no expansion {prefix}"));
  line.rsplit_once("weight ").unwrap().1.trim_end_matches(')').parse().unwrap()
}

#[test]
fn should_reach_the_invoker_builder_through_a_parameter_decorator_alias() {
  let fixture = web_fixture(&[]);

  let answer = answer(&fixture, EXPORT_TASK);

  let invoker = item(&answer.must_read, INVOKER)
    .or_else(|| item(&answer.skim, INVOKER))
    .unwrap_or_else(|| panic!("{:?}", keys(&answer.skim)));
  assert_eq!(
    (invoker.why.as_str(), invoker.uncertain),
    (
      "shares INPUT_SLOTS with the parameter decorator Reply of FeedController.feedExport: references INPUT_SLOTS [resolved]",
      false
    )
  );
}

#[test]
fn should_name_the_class_decorator_whose_metadata_a_function_reads() {
  let fixture = web_fixture(&[]);

  let answer = answer(&fixture, EXPORT_TASK);

  let shared: Vec<(&str, &str)> = shared_by_score(&answer).into_iter().map(|(key, why, _)| (key, why)).collect();
  assert_eq!(
    shared,
    [
      (
        INVOKER,
        "shares INPUT_SLOTS with the parameter decorator Reply of FeedController.feedExport: references INPUT_SLOTS [resolved]"
      ),
      (
        SORTER,
        "shares RESOURCE_KIND with the class decorator Resource of FeedController: references RESOURCE_KIND [resolved]"
      ),
    ]
  );
}

#[test]
fn should_weigh_parameter_and_class_decorator_consumers_by_their_own_factors() {
  let fixture = web_fixture(&[]);

  let answer = explained(&fixture, EXPORT_TASK);

  assert_eq!([INVOKER, SORTER].map(|to| expansion_weight(&answer, HANDLER, to)), [0.315, 0.225]);
}

#[test]
fn should_not_take_a_handler_that_applies_the_shared_function_as_a_decorator_for_a_consumer() {
  let fixture = web_fixture(&[]);

  let answer = explained(&fixture, EXPORT_TASK);

  let explain = answer.explain.as_ref().unwrap();
  let legacy: Vec<&String> = explain.expansions.iter().filter(|line| line.contains("LegacyController::rss")).collect();
  assert_eq!(legacy, Vec::<&String>::new());
}

#[test]
fn should_count_the_users_of_a_shared_function_outside_the_decorators_own_file() {
  let alias =
    "import { defineRoute, Verb } from '@corvid/web';\n\nexport function aliasRoute(path: string): MethodDecorator {\n  return defineRoute({ verb: Verb.Every, path });\n}\n";
  let fixture = web_fixture(&[("apps/portal/src/route-alias.ts", alias)]);

  let answer = answer(&fixture, EXPORT_TASK);

  let shared: Vec<(&str, &str)> = shared_by_score(&answer).into_iter().map(|(key, why, _)| (key, why)).collect();
  assert_eq!(
    shared,
    [
      (
        INVOKER,
        "shares INPUT_SLOTS with the parameter decorator Reply of FeedController.feedExport: references INPUT_SLOTS [resolved]"
      ),
      (
        "ts:corvid:corvid-portal:src/route-alias::aliasRoute#function",
        "shares defineRoute with the route decorator Fetch of FeedController.feedExport: calls defineRoute [resolved]"
      ),
      (
        SORTER,
        "shares RESOURCE_KIND with the class decorator Resource of FeedController: references RESOURCE_KIND [resolved]"
      ),
    ]
  );
}

#[test]
fn should_count_a_function_that_reads_another_variant_as_a_user_of_the_enum_the_route_decorator_references() {
  let head = "import { Verb } from '@corvid/web';\n\nexport function isHeadOnly(verb: string): boolean {\n  return verb === Verb.Probe;\n}\n";
  let fixture = web_fixture(&[("apps/portal/src/route-alias.ts", head)]);

  let answer = explained(&fixture, EXPORT_TASK);

  let prefix = format!("{HANDLER} -> ts:corvid:corvid-portal:src/route-alias::isHeadOnly#function (");
  let expansions: Vec<&String> = answer.explain.as_ref().unwrap().expansions.iter().filter(|line| line.starts_with(&prefix)).collect();
  assert_eq!(
    expansions,
    [&format!(
      "{prefix}shares Verb with the route decorator Fetch of FeedController.feedExport: references Verb.Probe [resolved], weight 0.079)"
    )]
  );
}

const AUDIT: &str = "import { MetaStore } from '@corvid/meta';\n\nexport const AUDIT_KEY = Symbol('corvid:web:audit');\n\nexport function Audit(tag: string): MethodDecorator {\n  return (_target, _property, descriptor) => MetaStore.merge(AUDIT_KEY, tag, descriptor.value as object);\n}\n";
const AUDIT_LOG: &str = "import { MetaStore } from '@corvid/meta';\n\nimport { AUDIT_KEY } from './audit';\n\nexport function auditTag(method: object): string | undefined {\n  return MetaStore.read<string>(AUDIT_KEY, method);\n}\n";
const ONE_LINE_AUDIT: &str = "import { Audit, Fetch, Reply, type ReplyHandle, Resource } from '@corvid/web';\n\n@Resource()\nexport class FeedController {\n  @Fetch('/feeds/export') @Audit('feeds')\n  async feedExport(@Reply() reply: ReplyHandle) {\n    reply.send(await Promise.resolve('<feed/>'));\n  }\n}\n";
const WEB_INDEX_WITH_AUDIT: &str = "export * from './verb';\nexport * from './route.options';\nexport * from './route.decorators';\nexport * from './resource.decorator';\nexport * from './params';\nexport * from './reply';\nexport * from './web-router';\nexport * from './audit';\n";

fn shares_of(answer: &ContextAnswer, from: &str) -> Vec<String> {
  let prefix = format!("{from} -> ");
  answer
    .explain
    .as_ref()
    .unwrap()
    .expansions
    .iter()
    .filter(|line| line.starts_with(&prefix) && line.contains(" (shares "))
    .map(|line| line[prefix.len()..].to_owned())
    .collect()
}

#[test]
fn should_not_take_a_decorator_on_the_route_decorators_line_for_the_route_decorator() {
  let fixture = web_fixture(&[
    ("packages/web/src/index.ts", WEB_INDEX_WITH_AUDIT),
    ("packages/web/src/audit.ts", AUDIT),
    ("packages/web/src/audit-log.ts", AUDIT_LOG),
    ("apps/portal/src/feed.controller.ts", ONE_LINE_AUDIT),
  ]);

  let answer = explained(&fixture, EXPORT_TASK);

  let audit: Vec<String> = shares_of(&answer, HANDLER).into_iter().filter(|line| line.contains("auditTag")).collect();
  assert_eq!(
    audit,
    [
      "ts:corvid:@corvid/web:src/audit-log::auditTag#function (shares AUDIT_KEY with the decorator Audit of FeedController.feedExport: references AUDIT_KEY [resolved], weight 0.315)"
    ]
  );
}

#[test]
fn should_keep_a_one_line_decorated_reader_whose_body_shares_the_key() {
  let bound = "export function Bound(): MethodDecorator {\n  return (_target, _property, descriptor) => descriptor;\n}\n";
  let stats = "import { MetaStore } from '@corvid/meta';\n\nimport { Bound } from './bound';\nimport { INPUT_SLOTS } from './params';\n\nexport class SlotStats {\n  @Bound() slotCount(method: object): number { return MetaStore.read<unknown[]>(INPUT_SLOTS, method)?.length ?? 0; }\n}\n";
  let fixture = web_fixture(&[("packages/web/src/bound.ts", bound), ("packages/web/src/slot-stats.ts", stats)]);

  let answer = explained(&fixture, EXPORT_TASK);

  let stats: Vec<String> = shares_of(&answer, HANDLER).into_iter().filter(|line| line.contains("SlotStats")).collect();
  assert_eq!(
    stats,
    [
      "ts:corvid:@corvid/web:src/slot-stats::SlotStats::slotCount#method (shares INPUT_SLOTS with the parameter decorator Reply of FeedController.feedExport: references INPUT_SLOTS [resolved], weight 0.158)"
    ]
  );
}

#[test]
fn should_hide_a_router_function_that_another_decorators_implementation_calls() {
  let resource = "import { MetaStore } from '@corvid/meta';\n\nimport { WebRouter } from './web-router';\n\nexport const RESOURCE_KIND = Symbol('corvid:web:resource');\n\nexport function Resource(options: { order?: number } = {}): ClassDecorator {\n  return target => {\n    MetaStore.merge(RESOURCE_KIND, { order: options.order ?? 0 }, target);\n    WebRouter.warm(new WebRouter(), target);\n  };\n}\n";
  let router = WEB_FILES.iter().find(|(path, _)| *path == "packages/web/src/web-router.ts").unwrap().1.replace(
    "  private sortResources(",
    "  static warm(router: WebRouter, unit: object): void {\n    router.buildInvoker({ unit, method: () => undefined, options: {} });\n  }\n\n  private sortResources(",
  );
  let fixture = web_fixture(&[("packages/web/src/resource.decorator.ts", resource), ("packages/web/src/web-router.ts", &router)]);

  let answer = explained(&fixture, EXPORT_TASK);

  assert_eq!(
    shares_of(&answer, HANDLER),
    [
      "ts:corvid:@corvid/web:src/web-router::WebRouter::mount#method (shares WebRouter.buildInvoker with the class decorator Resource of FeedController: calls WebRouter.buildInvoker [resolved], weight 0.225)",
      "ts:corvid:@corvid/web:src/web-router::WebRouter::sortResources#method (shares RESOURCE_KIND with the class decorator Resource of FeedController: references RESOURCE_KIND [resolved], weight 0.225)",
    ]
  );
}

#[test]
fn should_follow_every_decorator_of_a_sibling_routes_handler_too() {
  let feeds = "import { Fetch, Payload, Reply, type ReplyHandle, Resource, Store } from '@corvid/web';\n\n@Resource()\nexport class FeedController {\n  @Fetch('/feeds/export')\n  async feedExport(@Reply() reply: ReplyHandle) {\n    reply.send(await Promise.resolve('<feed/>'));\n  }\n\n  @Store('/feeds/subscribe')\n  subscribe(@Payload() payload: string) {\n    return payload;\n  }\n}\n";
  let fixture = web_fixture(&[("apps/portal/src/feed.controller.ts", feeds)]);

  let answer = explained(&fixture, EXPORT_TASK);

  let sibling = "ts:corvid:corvid-portal:src/feed.controller::FeedController::subscribe#method";
  assert_eq!(
    shares_of(&answer, sibling),
    [
      "ts:corvid:@corvid/web:src/web-router::WebRouter::buildInvoker#method (shares INPUT_SLOTS with the parameter decorator Payload of FeedController.subscribe: references INPUT_SLOTS [resolved], weight 0.158)",
      "ts:corvid:@corvid/web:src/web-router::WebRouter::sortResources#method (shares RESOURCE_KIND with the class decorator Resource of FeedController: references RESOURCE_KIND [resolved], weight 0.113)",
    ]
  );
}

#[test]
fn should_name_the_only_endpoint_of_a_route_mentioned_without_a_method() {
  let fixture = web_fixture(&[]);

  let answer = answer(&fixture, "`/feeds/export` answers 204 with nothing in it instead of the Atom feed");

  let endpoint = item(&answer.skim, "endpoint:corvid:GET /feeds/export").unwrap_or_else(|| panic!("{:?}", keys(&answer.skim)));
  assert_eq!(endpoint.why, "the endpoint GET /feeds/export named in the task");
  assert!(item(&answer.skim, INVOKER).or_else(|| item(&answer.must_read, INVOKER)).is_some());
}

#[test]
fn should_name_no_endpoint_of_a_route_with_two_methods_mentioned_without_one() {
  let health = "import { Fetch, Probe, Reply, type ReplyHandle, Resource } from '@corvid/web';\n\n@Resource({ order: -1 })\nexport class HealthController {\n  @Fetch('/health/live')\n  live(@Reply() reply: ReplyHandle) {\n    reply.send('ok');\n  }\n\n  @Probe('/health/live')\n  probe(@Reply() reply: ReplyHandle) {\n    reply.send('');\n  }\n}\n";
  let fixture = web_fixture(&[("apps/portal/src/health.controller.ts", health)]);

  let answer = answer(&fixture, "`/health/live` answers 503 during deploys");

  let endpoints: Vec<(&str, &str)> = [&answer.must_read, &answer.skim]
    .into_iter()
    .flat_map(|items| items.iter())
    .filter(|item| item.entity.key.starts_with("endpoint:"))
    .map(|item| (item.entity.key.as_str(), item.why.as_str()))
    .collect();
  assert_eq!(
    endpoints,
    [
      ("endpoint:corvid:GET /health/live", "same route as /health/live (another method)"),
      ("endpoint:corvid:HEAD /health/live", "same route as /health/live (another method)"),
    ]
  );
}
