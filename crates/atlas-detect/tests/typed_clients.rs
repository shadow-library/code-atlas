mod common;

use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::{ContentHash, Detection, Detector, EdgeKind, NodeRef};
use atlas_detect::{DECLARED_CLIENTS_VERSION, DeclaredClientsDetector, DecoratorFamily, DiagnosingDetector, HttpClientDetector};
use common::{Analyzed, analyze, lines, render, run, sorted};
use serde_json::{Value, json};

const PINNED_DECLARED: (u32, &str, &str) = (
  1,
  "ee5330dff50f306d7474d7f6cceedae814acd5b98cefc0cd3b487048e8ccd3b7",
  "71f471e3db33e64cac9f3a6113675803c557b18b07d3a3da15b1325fab8fde39",
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

#[test]
fn should_detect_every_declared_client_call_of_the_corpus() {
  let detector = DeclaredClientsDetector::new(&[acme_books()]);
  let mut found = Vec::new();
  for (_, analyzed) in books_web() {
    found.extend(render(&analyzed.facts, &run(&detector, &analyzed).0));
  }
  assert_eq!(
    sorted(found),
    lines(&[
      "client:books-web:DELETE /books/{} [syntactic]",
      "client:books-web:DELETE {}/shelves/{} [heuristic 0.70]",
      "client:books-web:GET /books/{} [syntactic]",
      "client:books-web:POST /books [syntactic]",
      "makes_request addBook -> client:books-web:POST /books [syntactic]",
      "makes_request loadBook -> client:books-web:GET /books/{} [syntactic]",
      "makes_request removeBook -> client:books-web:DELETE /books/{} [syntactic]",
      "makes_request removeShelf -> client:books-web:DELETE {}/shelves/{} [heuristic 0.70]",
    ])
  );
}

#[test]
fn should_change_declared_client_output_over_the_corpus_only_with_a_version_bump() {
  let family = acme_books();
  let detector = DeclaredClientsDetector::new(std::slice::from_ref(&family));
  let mut inputs = serde_json::to_vec(&family).unwrap();
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
