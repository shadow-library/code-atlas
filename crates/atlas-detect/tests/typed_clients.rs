mod common;

use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::{ContentHash, Detection, Detector, EdgeKind, NodeRef};
use atlas_detect::{DECLARED_CLIENTS_VERSION, DeclaredClientsDetector, DecoratorFamily, DiagnosingDetector, HttpClientDetector};
use common::{Analyzed, analyze, lines, render, run, sorted};
use serde_json::{Value, json};

const PINNED_DECLARED: (u32, &str, &str) = (
  3,
  "e8b6188bcd7a65ed0e2909d0188dcc0ce872990c062d210b7fcfad11f727ca57",
  "54fe8b5c8cde43605809ebf01beb9f9bac46b2ac6c8a5b892505b82417b20128",
);

fn detect(detector: &dyn DiagnosingDetector, path: &str, source: &str) -> (Vec<String>, Detection) {
  let analyzed = analyze("web", path, source);
  let (detection, _) = run(detector, &analyzed);
  (sorted(render(&analyzed.facts, &detection)), detection)
}

fn request_attrs(detection: &Detection, client: &str) -> Value {
  let edge = detection
    .edges
    .iter()
    .find(|edge| edge.kind == EdgeKind::MakesRequest && matches!(&edge.to, NodeRef::Key(key) if key.to_string() == client))
    .unwrap_or_else(|| panic!("no makes_request edge to {client}"));
  serde_json::to_value(&edge.attrs).unwrap()
}

fn family(name: &str, text: &str) -> DecoratorFamily {
  let parsed = DecoratorFamily::parse(name, text).unwrap();
  assert_eq!(parsed.warnings, Vec::<String>::new());
  parsed.family
}

fn declared(text: &str) -> DeclaredClientsDetector {
  DeclaredClientsDetector::new(&[family("acme-books", text)])
}

const OPENAPI_FETCH: &str = r#"import createClient from 'openapi-fetch';
import type { paths } from '@acme/books-api/openapi';

const books = createClient<paths>({ baseUrl: process.env.BOOKS_API_URL, headers: { Authorization: `Bearer ${token()}` } });

export async function loadBook(id: string) {
  return books.GET('/books/{id}', { params: { path: { id } } });
}

export async function addReview(id: string, body: unknown) {
  return books.POST('/books/{bookId}/reviews', { params: { path: { bookId: id } }, body, headers: { 'x-api-key': key() } });
}

export function searchBooks(term: string) {
  return books.GET(`/books/search/${term}`, { params: { query: { limit: 10 } } });
}

export function notAClient(cache: Map<string, string>, books: { GET(path: string): void }) {
  cache.get('/books/{id}');
  books.GET('/books/{id}');
}
"#;

#[test]
fn should_detect_openapi_fetch_calls_on_a_create_client_binding_and_rewrite_template_segments() {
  let (lines_found, detection) = detect(&HttpClientDetector, "apps/web/src/books.ts", OPENAPI_FETCH);
  assert_eq!(
    lines_found,
    lines(&[
      "client:web:GET {}/books/search/{} [heuristic 0.70]",
      "client:web:GET {}/books/{} [heuristic 0.70]",
      "client:web:POST {}/books/{}/reviews [heuristic 0.70]",
      "makes_request addReview -> client:web:POST {}/books/{}/reviews [heuristic 0.70]",
      "makes_request loadBook -> client:web:GET {}/books/{} [heuristic 0.70]",
      "makes_request searchBooks -> client:web:GET {}/books/search/{} [heuristic 0.70]",
    ])
  );
  assert_eq!(
    request_attrs(&detection, "client:web:GET {}/books/{}"),
    json!({
      "library": "openapi-fetch",
      "method_source": "literal",
      "base_source": "env:BOOKS_API_URL",
      "auth_hint": ["bearer"],
      "path_template": "/books/{id}",
      "types_module": "@acme/books-api/openapi",
    })
  );
  let review = request_attrs(&detection, "client:web:POST {}/books/{}/reviews");
  assert_eq!(review["auth_hint"], json!(["api_key:x-api-key", "bearer"]));
  assert_eq!(review["path_template"], json!("/books/{bookId}/reviews"));
  assert_eq!(request_attrs(&detection, "client:web:GET {}/books/search/{}").get("path_template"), None);
}

#[test]
fn should_follow_an_aliased_factory_with_a_static_base_and_relative_generated_types() {
  let source = r#"import makeClient from 'openapi-fetch';
import type * as BooksApi from '../../../books-api/src/generated/openapi';

export function loadAuthor(id: string) {
  const api = makeClient<BooksApi.paths>({ baseUrl: 'https://books.acme.test/v1' });
  return api.GET('/authors/{authorId}');
}
"#;
  let (lines_found, detection) = detect(&HttpClientDetector, "apps/web/src/api/authors.ts", source);
  assert_eq!(
    lines_found,
    lines(&[
      "client:web:GET https://books.acme.test/v1/authors/{} [syntactic]",
      "makes_request loadAuthor -> client:web:GET https://books.acme.test/v1/authors/{} [syntactic]",
    ])
  );
  let attrs = request_attrs(&detection, "client:web:GET https://books.acme.test/v1/authors/{}");
  assert_eq!(attrs["base_source"], json!("literal:https://books.acme.test"));
  assert_eq!(attrs["types_module"], json!("../../../books-api/src/generated/openapi"));
  assert_eq!(attrs["types_path"], json!("apps/books-api/src/generated/openapi"));
}

#[test]
fn should_ignore_lookalike_factories_and_clients_not_created_in_the_file() {
  let source = r#"import { createClient } from '@acme/cache';
import { booksApi } from './api/client';

const cache = createClient();

export function load(id: string) {
  cache.GET('/books/{id}');
  booksApi.GET('/books/{id}');
}
"#;
  assert_eq!(detect(&HttpClientDetector, "apps/web/src/load.ts", source).0, Vec::<String>::new());
}

const BOOKS_CLIENT: &str = "[client]\nfiles = [\"apps/web/src/api/*.ts\"]\nmodules = [\"@acme/books-client\"]\nexports = [\"booksApi\", \"default\"]\nfactories = [\"createBooksClient\"]\nmethods = [\"get\", \"post\", \"delete\"]\ntarget_package = \"@acme/books-api\"\n";

#[test]
fn should_detect_declared_clients_imported_by_name_alias_default_or_namespace() {
  let source = r#"import { booksApi as api, formatTitle } from './api/client';
import legacy from '../src/api/legacy.js';
import * as clients from '@acme/books-client';

export const loadBook = (id: string) => api.get('/books/{id}', { headers: { Authorization: `Bearer ${id}` } });
export const removeBook = (id: string) => legacy.delete(`/books/${id}`);
export const addBook = (book: unknown) => clients.booksApi.post('/books', book);
export const formatted = () => formatTitle.get('/titles');
export const unlisted = () => api.put('/books/{id}');
"#;
  let (lines_found, detection) = detect(&declared(BOOKS_CLIENT), "apps/web/src/books.ts", source);
  assert_eq!(
    lines_found,
    lines(&[
      "client:web:DELETE /books/{} [syntactic]",
      "client:web:GET /books/{} [syntactic]",
      "client:web:POST /books [syntactic]",
      "makes_request addBook -> client:web:POST /books [syntactic]",
      "makes_request loadBook -> client:web:GET /books/{} [syntactic]",
      "makes_request removeBook -> client:web:DELETE /books/{} [syntactic]",
    ])
  );
  assert_eq!(
    request_attrs(&detection, "client:web:GET /books/{}"),
    json!({
      "library": "acme-books",
      "method_source": "literal",
      "auth_hint": ["bearer"],
      "path_template": "/books/{id}",
      "target_package": "@acme/books-api",
      "client_module": "apps/web/src/api/client",
      "client_export": "booksApi",
    })
  );
}

#[test]
fn should_detect_declared_factory_clients_and_express_style_paths() {
  let text = "[client]\nmodules = [\"@acme/books-client\"]\nfactories = [\"createBooksClient\"]\nmethods = [\"fetchOne\"]\n";
  assert!(DecoratorFamily::parse("acme-books", text).is_err());
  let text = "[client]\nmodules = [\"@acme/books-client\"]\nfactories = [\"createBooksClient\"]\nstyle = \"express\"\npath = { argument = 0, key = \"url\" }\n";
  let source = r#"import { createBooksClient } from '@acme/books-client';

export async function loadShelf(shelf: string) {
  const client = await createBooksClient({ baseURL: 'https://books.acme.test' });
  return client.get({ url: '/shelves/:shelfId/books', headers: { cookie: shelf } });
}

export function notTheFactory(createBooksClient: () => { get(path: string): void }) {
  const other = createBooksClient();
  return other.get('/shelves/:shelfId');
}
"#;
  let (lines_found, detection) = detect(&declared(text), "apps/web/src/shelves.ts", source);
  assert_eq!(
    lines_found,
    lines(&[
      "client:web:GET https://books.acme.test/shelves/{}/books [syntactic]",
      "makes_request loadShelf -> client:web:GET https://books.acme.test/shelves/{}/books [syntactic]",
    ])
  );
  let attrs = request_attrs(&detection, "client:web:GET https://books.acme.test/shelves/{}/books");
  assert_eq!(attrs["path_template"], json!("/shelves/:shelfId/books"));
  assert_eq!(attrs["auth_hint"], json!(["cookie:*"]));
}

#[test]
fn should_not_match_shadowed_bindings_unlisted_modules_or_files_outside_the_globs() {
  let source = r#"import { booksApi } from './api/client';
import { booksApi as other } from './elsewhere/client';
import { booksApi as nested } from './api/v2/client';

export function shadowed(booksApi: { get(path: string): void }) {
  return booksApi.get('/books/{id}');
}

export function local() {
  const booksApi = { get: (path: string) => path };
  return booksApi.get('/books/{id}');
}

export const fromElsewhere = () => other.get('/books/{id}');
export const fromNested = () => nested.get('/books/{id}');
"#;
  assert_eq!(detect(&declared(BOOKS_CLIENT), "apps/web/src/books.ts", source).0, Vec::<String>::new());
}

#[test]
fn should_version_the_declared_client_detector_by_its_client_shapes() {
  let one = declared(BOOKS_CLIENT);
  let changed = declared(&BOOKS_CLIENT.replace("\"delete\"", "\"put\""));
  let server = family(
    "acme-http",
    "packages = [\"@acme/http\"]\n[controller]\ndecorators = [\"Api\"]\n[routes.methods]\nRead = \"GET\"\n",
  );

  assert_eq!(one.producer(), declared(BOOKS_CLIENT).producer());
  assert_eq!(one.producer().name, "atlas-detect.http-clients-declared");
  assert_ne!(one.producer().version, changed.producer().version);
  assert_eq!(
    DeclaredClientsDetector::new(&[family("acme-books", BOOKS_CLIENT), server.clone()]).producer(),
    one.producer()
  );
  assert!(DeclaredClientsDetector::new(&[server]).is_empty());
}

fn corpus() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/family-corpus")
}

fn corpus_files(dir: &Path, root: &Path, files: &mut Vec<(String, Analyzed)>) {
  let mut entries: Vec<PathBuf> = fs::read_dir(dir).unwrap().flatten().map(|entry| entry.path()).collect();
  entries.sort();
  for path in entries {
    if path.is_dir() {
      corpus_files(&path, root, files);
      continue;
    }
    let relative = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
    let source = fs::read_to_string(&path).unwrap();
    let analyzed = analyze("books-web", &relative, &source);
    files.push((relative, analyzed));
  }
}

fn books_web() -> Vec<(String, Analyzed)> {
  let root = corpus().join("books-web");
  let mut files = Vec::new();
  corpus_files(&root, &root, &mut files);
  files
}

fn acme_books() -> DecoratorFamily {
  family("acme-books", &fs::read_to_string(corpus().join("acme-books.toml")).unwrap())
}

fn corpus_clients() -> Vec<DecoratorFamily> {
  let kits = ["acme-kit", "acme-kit-admin"].map(|name| family(name, &fs::read_to_string(corpus().join(format!("{name}.toml"))).unwrap()));
  std::iter::once(acme_books()).chain(kits).collect()
}

#[test]
fn should_detect_every_declared_client_call_of_the_corpus() {
  let detector = DeclaredClientsDetector::new(&corpus_clients());
  let mut found = Vec::new();
  for (_, analyzed) in books_web() {
    found.extend(render(&analyzed.facts, &run(&detector, &analyzed).0));
  }
  assert_eq!(
    sorted(found),
    lines(&[
      "client:books-web:DELETE /books/{} [syntactic]",
      "client:books-web:DELETE /current [syntactic]",
      "client:books-web:DELETE {}/shelves/{} [heuristic 0.70]",
      "client:books-web:GET /books/{} [syntactic]",
      "client:books-web:GET /current [syntactic]",
      "client:books-web:GET /shelves [syntactic]",
      "client:books-web:HEAD /api/v2/ping [syntactic]",
      "client:books-web:PATCH /shelves/{} [syntactic]",
      "client:books-web:POST /books [syntactic]",
      "makes_request addBook -> client:books-web:POST /books [syntactic]",
      "makes_request currentSession -> client:books-web:GET /current [syntactic]",
      "makes_request endSession -> client:books-web:DELETE /current [syntactic]",
      "makes_request listShelves -> client:books-web:GET /shelves [syntactic]",
      "makes_request loadBook -> client:books-web:GET /books/{} [syntactic]",
      "makes_request pingShelves -> client:books-web:HEAD /api/v2/ping [syntactic]",
      "makes_request removeBook -> client:books-web:DELETE /books/{} [syntactic]",
      "makes_request removeShelf -> client:books-web:DELETE {}/shelves/{} [heuristic 0.70]",
      "makes_request renameShelf -> client:books-web:PATCH /shelves/{} [syntactic]",
      "other:typed_client:books-web:src/admin/transport.ts#AdminRequest [syntactic]",
      "other:typed_client:books-web:src/admin/transport.ts#adminKit.audit [syntactic]",
      "other:typed_client:books-web:src/admin/transport.ts#adminKit.users [syntactic]",
      "other:typed_client:books-web:src/transport.ts#ShelfRequest [syntactic]",
      "other:typed_client:books-web:src/transport.ts#kitClient.session [syntactic]",
      "other:typed_client:books-web:src/transport.ts#kitClient.shelves [syntactic]",
    ])
  );
}

#[test]
fn should_attribute_each_corpus_transport_definition_to_the_family_owning_its_file() {
  let detector = DeclaredClientsDetector::new(&corpus_clients());
  let mut owners = Vec::new();
  for (path, analyzed) in books_web() {
    let (detection, _) = run(&detector, &analyzed);
    owners.extend(
      entity_attrs(&detection)
        .into_iter()
        .map(|(key, attrs)| (key, attrs["library"].as_str().unwrap().to_owned())),
    );
    assert!(path.ends_with("transport.ts") || entity_attrs(&detection).is_empty(), "{path}");
  }
  owners.sort();
  let libraries: Vec<(&str, &str)> = owners
    .iter()
    .map(|(key, library)| (if key.contains("/admin/") { "admin" } else { "src" }, library.as_str()))
    .collect();
  assert_eq!(
    libraries,
    vec![
      ("admin", "acme-kit-admin"),
      ("admin", "acme-kit-admin"),
      ("admin", "acme-kit-admin"),
      ("src", "acme-kit"),
      ("src", "acme-kit"),
      ("src", "acme-kit")
    ]
  );
}

#[test]
fn should_change_declared_client_output_over_the_corpus_only_with_a_version_bump() {
  let families = corpus_clients();
  let detector = DeclaredClientsDetector::new(&families);
  let mut inputs = serde_json::to_vec(&families).unwrap();
  let mut outputs = Vec::new();
  for (path, analyzed) in books_web() {
    inputs.extend(serde_json::to_vec(&analyzed.facts).unwrap());
    inputs.push(b'\n');
    let (detection, diagnostics) = run(&detector, &analyzed);
    let mut diagnostics: Vec<String> = diagnostics.iter().map(|diagnostic| serde_json::to_string(diagnostic).unwrap()).collect();
    diagnostics.sort();
    outputs.extend(format!("{path}\0").into_bytes());
    outputs.extend(serde_json::to_vec(&detection).unwrap());
    outputs.extend(serde_json::to_vec(&diagnostics).unwrap());
    outputs.push(b'\n');
  }
  let actual = (DECLARED_CLIENTS_VERSION, ContentHash::of(&inputs).to_string(), ContentHash::of(&outputs).to_string());
  let advice = match actual.1 == PINNED_DECLARED.1 {
    false => "the client corpus, its family file or the analyzers changed, so re-pin",
    true if actual.0 == PINNED_DECLARED.0 => "the declared-client output over unchanged inputs changed: bump DECLARED_CLIENTS_VERSION, then re-pin",
    true => "DECLARED_CLIENTS_VERSION changed: re-pin",
  };
  assert_eq!((actual.0, actual.1.as_str(), actual.2.as_str()), PINNED_DECLARED, "{advice}");
}

#[test]
fn should_record_exported_openapi_fetch_clients_with_their_base_and_generated_types() {
  let source = r#"import createClient from 'openapi-fetch';
import type { paths } from '../../../books-api/src/generated/openapi';

export const booksApi = createClient<paths>({ baseUrl: `${import.meta.env.VITE_BOOKS_URL}/api/` });

export const staticApi = createClient<paths>({ baseUrl: '/api/' });

const internalApi = createClient<paths>();
"#;
  let (lines_found, detection) = detect(&HttpClientDetector, "apps/web/src/api/index.ts", source);
  assert_eq!(
    lines_found,
    lines(&[
      "other:typed_client:web:apps/web/src/api/index.ts#booksApi [syntactic]",
      "other:typed_client:web:apps/web/src/api/index.ts#staticApi [syntactic]",
    ])
  );
  let attrs = |name: &str| {
    let entity = detection.entities.iter().find(|entity| entity.key.to_string().ends_with(name)).unwrap();
    serde_json::to_value(&entity.attrs).unwrap()
  };
  assert_eq!(
    attrs("#booksApi"),
    json!({
      "library": "openapi-fetch",
      "export": "booksApi",
      "modules": ["apps/web/src/api/index", "apps/web/src/api"],
      "base": "{}/api",
      "base_source": "expr",
      "types_module": "../../../books-api/src/generated/openapi",
      "types_path": "apps/books-api/src/generated/openapi",
    })
  );
  assert_eq!(attrs("#staticApi")["base"], json!("/api"));
  assert_eq!(attrs("#staticApi").get("base_source"), None);
}

const KIT_CLIENT: &str = "[client]\nfiles = [\"apps/web/src/transport.ts\"]\nexports = [\"APIRequest\", \"apiClient\"]\nbase = \"/api\"\n[client.surfaces]\nmodules = [\"@acme/http-kit\"]\nfactory = \"createGatewayClient\"\noption = \"surfaces\"\n";

const TRANSPORT: &str = r#"import { createGatewayClient } from '@acme/http-kit';
import { ssr } from './env';

const AUTH_SURFACES = { auth: '/api/auth/', admin: `/api/admin` } as const;

export const apiClient = createGatewayClient({ surfaces: { v1: '/api/v1', auth: `${process.env.AUTH_ORIGIN}/api/auth/`, ...extra, [KEY]: '/k' }, ssr });
export const authClient = await createGatewayClient<Surfaces>({ surfaces: AUTH_SURFACES });
export const APIRequest = apiClient.v1;
export const AdminRequest: Surface = authClient.admin as Surface;
export const Missing = apiClient.v2;
export const Called = apiClient.v1.withAuth();
export const Indexed = apiClient['v1'];
const internal = createGatewayClient({ surfaces: { v9: '/v9' } });
export const local = internal.v9;

export const session = () => apiClient.auth.get('/session');
export const users = () => APIRequest.get('/admin/users').query({ page: 1 }).execute();
"#;

fn entity_attrs(detection: &Detection) -> Vec<(String, Value)> {
  detection
    .entities
    .iter()
    .filter(|entity| entity.key.to_string().starts_with("other:typed_client:"))
    .map(|entity| (entity.key.to_string(), serde_json::to_value(&entity.attrs).unwrap()))
    .collect()
}

#[test]
fn should_record_each_surface_of_an_exported_container_and_each_exported_member_as_a_client_definition() {
  let (lines_found, detection) = detect(&declared(KIT_CLIENT), "apps/web/src/transport.ts", TRANSPORT);
  assert_eq!(
    lines_found,
    lines(&[
      "client:web:GET /api/v1/admin/users [syntactic]",
      "client:web:GET {}/api/auth/session [heuristic 0.70]",
      "makes_request session -> client:web:GET {}/api/auth/session [heuristic 0.70]",
      "makes_request users -> client:web:GET /api/v1/admin/users [syntactic]",
      "other:typed_client:web:apps/web/src/transport.ts#APIRequest [syntactic]",
      "other:typed_client:web:apps/web/src/transport.ts#AdminRequest [syntactic]",
      "other:typed_client:web:apps/web/src/transport.ts#apiClient.auth [syntactic]",
      "other:typed_client:web:apps/web/src/transport.ts#apiClient.v1 [syntactic]",
      "other:typed_client:web:apps/web/src/transport.ts#authClient.admin [syntactic]",
      "other:typed_client:web:apps/web/src/transport.ts#authClient.auth [syntactic]",
      "other:typed_client:web:apps/web/src/transport.ts#local [syntactic]",
    ])
  );
  let module = json!(["apps/web/src/transport"]);
  let definition = |export: &str, surface: &str, base: &str| json!({ "library": "acme-books", "export": export, "modules": module, "surface": surface, "base": base });
  let mut auth = definition("apiClient.auth", "auth", "{}/api/auth");
  auth["base_source"] = json!("env:AUTH_ORIGIN");
  assert_eq!(
    entity_attrs(&detection),
    [
      ("other:typed_client:web:apps/web/src/transport.ts#apiClient.auth".to_owned(), auth),
      (
        "other:typed_client:web:apps/web/src/transport.ts#apiClient.v1".to_owned(),
        definition("apiClient.v1", "v1", "/api/v1")
      ),
      (
        "other:typed_client:web:apps/web/src/transport.ts#authClient.admin".to_owned(),
        definition("authClient.admin", "admin", "/api/admin")
      ),
      (
        "other:typed_client:web:apps/web/src/transport.ts#authClient.auth".to_owned(),
        definition("authClient.auth", "auth", "/api/auth")
      ),
      (
        "other:typed_client:web:apps/web/src/transport.ts#APIRequest".to_owned(),
        definition("APIRequest", "v1", "/api/v1")
      ),
      (
        "other:typed_client:web:apps/web/src/transport.ts#AdminRequest".to_owned(),
        definition("AdminRequest", "admin", "/api/admin")
      ),
      ("other:typed_client:web:apps/web/src/transport.ts#local".to_owned(), definition("local", "v9", "/v9")),
    ]
  );
  assert_eq!(request_attrs(&detection, "client:web:GET /api/v1/admin/users").get("client_base"), None);
  assert_eq!(request_attrs(&detection, "client:web:GET {}/api/auth/session")["base_source"], json!("env:AUTH_ORIGIN"));
}

const TRANSPORT_CALLERS: &str = r#"import { APIRequest, apiClient as client, helpers } from './transport';
import * as transport from './transport';

export const loadUsers = (q: string) => APIRequest.get('/admin/users').query(q).execute<User[]>();
export const createDraft = (id: string) => APIRequest.post(`/projects/${id}/drafts`).body({}).execute();
export const loadSession = () => client.auth.get('/session');
export const endSession = () => transport.apiClient.auth.delete('/session');
export const checkHealth = () => transport.APIRequest.head('/health');
export const notAClient = () => helpers.cache.get('/cached');
export const findUser = (id: string) => APIRequest.get(`/${id}`).execute();
"#;

#[test]
fn should_record_the_export_or_container_member_a_call_goes_through_for_the_link_join() {
  let (lines_found, detection) = detect(&declared(KIT_CLIENT), "apps/web/src/users.ts", TRANSPORT_CALLERS);
  assert_eq!(
    lines_found,
    lines(&[
      "client:web:DELETE /session [syntactic]",
      "client:web:GET /admin/users [syntactic]",
      "client:web:GET /session [syntactic]",
      "client:web:GET /{} [syntactic]",
      "client:web:HEAD /health [syntactic]",
      "client:web:POST /projects/{}/drafts [syntactic]",
      "makes_request checkHealth -> client:web:HEAD /health [syntactic]",
      "makes_request createDraft -> client:web:POST /projects/{}/drafts [syntactic]",
      "makes_request endSession -> client:web:DELETE /session [syntactic]",
      "makes_request findUser -> client:web:GET /{} [syntactic]",
      "makes_request loadSession -> client:web:GET /session [syntactic]",
      "makes_request loadUsers -> client:web:GET /admin/users [syntactic]",
    ])
  );
  let joined = |client: &str| {
    let attrs = request_attrs(&detection, client);
    (attrs["client_module"].clone(), attrs["client_export"].clone(), attrs["client_base"].clone())
  };
  let module = json!("apps/web/src/transport");
  assert_eq!(joined("client:web:GET /admin/users"), (module.clone(), json!("APIRequest"), json!("/api")));
  assert_eq!(joined("client:web:GET /session"), (module.clone(), json!("apiClient.auth"), json!("/api")));
  assert_eq!(joined("client:web:DELETE /session"), (module.clone(), json!("apiClient.auth"), json!("/api")));
  assert_eq!(joined("client:web:HEAD /health"), (module, json!("APIRequest"), json!("/api")));

  let without_surfaces = "[client]\nfiles = [\"apps/web/src/transport.ts\"]\nexports = [\"APIRequest\", \"apiClient\"]\n";
  let (direct_only, detection) = detect(&declared(without_surfaces), "apps/web/src/users.ts", TRANSPORT_CALLERS);
  assert_eq!(
    direct_only,
    lines(&[
      "client:web:GET /admin/users [syntactic]",
      "client:web:GET /{} [syntactic]",
      "client:web:HEAD /health [syntactic]",
      "client:web:POST /projects/{}/drafts [syntactic]",
      "makes_request checkHealth -> client:web:HEAD /health [syntactic]",
      "makes_request createDraft -> client:web:POST /projects/{}/drafts [syntactic]",
      "makes_request findUser -> client:web:GET /{} [syntactic]",
      "makes_request loadUsers -> client:web:GET /admin/users [syntactic]",
    ])
  );
  assert_eq!(request_attrs(&detection, "client:web:GET /admin/users").get("client_base"), None);

  let unjoinable = "import { booksApi } from '@acme/books-client';

export const findBook = (id: string) => booksApi.get(`/${id}`);
";
  let (lines_found, _) = detect(&declared(BOOKS_CLIENT), "apps/web/src/books.ts", unjoinable);
  assert_eq!(lines_found, Vec::<String>::new());
}

#[test]
fn should_not_take_surfaces_from_lookalike_factories_or_shadowed_containers() {
  let source = r#"import { createGatewayClient } from '@acme/other-kit';
import { createGatewayClient as kit } from '@acme/http-kit';

export const apiClient = createGatewayClient({ surfaces: { v1: '/api/v1' } });
export const wrongOption = kit({ bases: { v1: '/api/v1' } });

export function shadowed(apiClient: { v1: { get(path: string): void } }) {
  return apiClient.v1.get('/users');
}
"#;
  let (lines_found, _) = detect(&declared(KIT_CLIENT), "apps/web/src/transport.ts", source);
  assert_eq!(lines_found, Vec::<String>::new());
}

#[test]
fn should_follow_members_only_of_declared_containers() {
  let source = r#"import { APIRequest, apiClient } from './transport';
import * as transport from './transport';

export const loadSession = () => apiClient.auth.get('/session');
export const endSession = () => transport.apiClient.auth.delete('/session');
export const cached = () => APIRequest.cache.get('/users');
export const cachedViaNamespace = () => transport.APIRequest.cache.post('/users');
"#;
  let with_containers = KIT_CLIENT.replace("option = \"surfaces\"\n", "option = \"surfaces\"\ncontainers = [\"apiClient\"]\n");
  let (lines_found, _) = detect(&declared(&with_containers), "apps/web/src/session.ts", source);
  assert_eq!(
    lines_found,
    lines(&[
      "client:web:DELETE /session [syntactic]",
      "client:web:GET /session [syntactic]",
      "makes_request endSession -> client:web:DELETE /session [syntactic]",
      "makes_request loadSession -> client:web:GET /session [syntactic]",
    ])
  );
  let (without_containers, _) = detect(&declared(KIT_CLIENT), "apps/web/src/session.ts", source);
  assert_eq!(without_containers.len(), 8);
}

fn web_family(files: &str) -> String {
  format!(
    "[client]\nfiles = [\"{files}\"]\nexports = [\"APIRequest\"]\n[client.surfaces]\nmodules = [\"@acme/http-kit\"]\nfactory = \"createGatewayClient\"\noption = \"surfaces\"\n"
  )
}

const WEB_TRANSPORT: &str = r#"import { createGatewayClient } from '@acme/http-kit';

export const apiClient = createGatewayClient({ surfaces: { v1: '/api/v1' } });
export const APIRequest = apiClient.v1;
"#;

fn definitions(detector: &DeclaredClientsDetector, path: &str) -> Vec<(String, Value)> {
  let analyzed = analyze("web", path, WEB_TRANSPORT);
  let (detection, _) = run(detector, &analyzed);
  entity_attrs(&detection)
}

#[test]
fn should_record_a_shared_factory_definition_only_for_the_family_whose_files_match() {
  let detector = DeclaredClientsDetector::new(&[
    family("alpha-web", &web_family("apps/alpha/src/transport.ts")),
    family("beta-web", &web_family("apps/beta/src/transport.ts")),
  ]);
  for (path, owner) in [("apps/alpha/src/transport.ts", "alpha-web"), ("apps/beta/src/transport.ts", "beta-web")] {
    let entities = definitions(&detector, path);
    let mut keys: Vec<&str> = entities.iter().map(|(key, _)| key.as_str()).collect();
    keys.sort();
    assert_eq!(
      keys,
      vec![format!("other:typed_client:web:{path}#APIRequest"), format!("other:typed_client:web:{path}#apiClient.v1")]
    );
    assert!(entities.iter().all(|(_, attrs)| attrs["library"] == json!(owner)));
  }
  let entities = definitions(&detector, "apps/gamma/src/transport.ts");
  assert_eq!(entities, Vec::new());
}

#[test]
fn should_record_no_definition_claimed_by_two_families_and_report_the_conflict() {
  let detector = DeclaredClientsDetector::new(&[
    family("alpha-web", &web_family("apps/*/src/transport.ts")),
    family("beta-web", &web_family("apps/beta/src/transport.ts")),
  ]);
  let analyzed = analyze("web", "apps/beta/src/transport.ts", WEB_TRANSPORT);
  let (detection, diagnostics) = run(&detector, &analyzed);
  assert_eq!(entity_attrs(&detection), Vec::new());
  let messages: Vec<(&str, &str)> = diagnostics.iter().map(|diagnostic| (diagnostic.code.as_str(), diagnostic.message.as_str())).collect();
  assert_eq!(
    messages,
    vec![
      (
        "client-definition-conflict",
        "client definition other:typed_client:web:apps/beta/src/transport.ts#APIRequest is claimed by families alpha-web, beta-web; none records it"
      ),
      (
        "client-definition-conflict",
        "client definition other:typed_client:web:apps/beta/src/transport.ts#apiClient.v1 is claimed by families alpha-web, beta-web; none records it"
      ),
    ]
  );
  let entities = definitions(&detector, "apps/alpha/src/transport.ts");
  assert_eq!(entities.len(), 2);
  assert!(entities.iter().all(|(_, attrs)| attrs["library"] == json!("alpha-web")));
}

fn broad_family() -> String {
  "[client]\nmodules = [\"@acme/http-kit\"]\nexports = [\"APIRequest\"]\n[client.surfaces]\nmodules = [\"@acme/http-kit\"]\nfactory = \"createGatewayClient\"\noption = \"surfaces\"\n"
    .to_owned()
}

fn libraries(detector: &DeclaredClientsDetector, path: &str) -> (Vec<String>, Vec<String>) {
  let analyzed = analyze("web", path, WEB_TRANSPORT);
  let (detection, diagnostics) = run(detector, &analyzed);
  let mut libraries: Vec<String> = entity_attrs(&detection).iter().map(|(_, attrs)| attrs["library"].as_str().unwrap().to_owned()).collect();
  libraries.dedup();
  (libraries, diagnostics.into_iter().map(|diagnostic| diagnostic.code).collect())
}

#[test]
fn should_not_record_a_definition_for_a_family_that_only_imports_the_module_of_another() {
  let a_web = "[client]\nfiles = [\"apps/a-web/src/lib/apis/transport.ts\"]\nmodules = [\"@/lib/apis/transport\"]\nfactories = [\"createGatewayClient\"]\n";
  let b_web = "[client]\nfiles = [\"apps/b-web/src/lib/apis/transport.ts\"]\nfactories = [\"createGatewayClient\"]\n";
  let detector = DeclaredClientsDetector::new(&[family("a-web", a_web), family("b-web", b_web)]);
  let source = "import { createGatewayClient } from '@/lib/apis/transport';\n\nexport const uploadClient = createGatewayClient({ baseUrl: '/upload' });\n";
  let analyzed = analyze("web", "apps/b-web/src/lib/apis/upload.ts", source);
  let (detection, diagnostics) = run(&detector, &analyzed);
  assert_eq!(entity_attrs(&detection), Vec::new());
  assert_eq!(diagnostics, Vec::new());
}

#[test]
fn should_prefer_a_family_whose_files_match_over_one_without_files() {
  let detector = DeclaredClientsDetector::new(&[
    family("alpha-web", &web_family("apps/alpha/src/transport.ts")),
    family("beta-web", &web_family("apps/beta/src/transport.ts")),
    family("zz-broad", &broad_family()),
  ]);
  assert_eq!(libraries(&detector, "apps/alpha/src/transport.ts"), (vec!["alpha-web".to_owned()], Vec::new()));
  assert_eq!(libraries(&detector, "apps/beta/src/transport.ts"), (vec!["beta-web".to_owned()], Vec::new()));
  assert_eq!(libraries(&detector, "apps/gamma/src/transport.ts"), (vec!["zz-broad".to_owned()], Vec::new()));
}

#[test]
fn should_report_a_tie_between_families_without_files() {
  let detector = DeclaredClientsDetector::new(&[family("one-broad", &broad_family()), family("two-broad", &broad_family())]);
  let (libraries, codes) = libraries(&detector, "apps/gamma/src/transport.ts");
  assert_eq!(libraries, Vec::<String>::new());
  assert_eq!(codes, vec!["client-definition-conflict"; 2]);
}

#[test]
fn should_name_the_tied_families_of_a_conflict_diagnostic() {
  let detector = DeclaredClientsDetector::new(&[family("one-broad", &broad_family()), family("two-broad", &broad_family())]);
  let analyzed = analyze("web", "apps/gamma/src/transport.ts", WEB_TRANSPORT);
  let (_, diagnostics) = run(&detector, &analyzed);
  assert_eq!(atlas_detect::conflicting_families(&diagnostics[0]), vec!["one-broad", "two-broad"]);
}
