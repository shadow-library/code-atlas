mod common;

use atlas_core::{Certainty, EdgeKind, EntityKind, RefKind};
use atlas_scip::{ScipError, ScipIngestContext, ingest};
use common::*;
use scip::types::PositionEncoding::{UTF8CodeUnitOffsetFromLineStart as Utf8, UTF16CodeUnitOffsetFromLineStart as Utf16, UnspecifiedPositionEncoding};

const NESTED: &str = "class Outer {\n  helper() {\n    return load(1);\n  }\n}\nfunction load(n) { return n; }\n";

fn nested_context() -> ScipIngestContext {
  let outer = symbol(NESTED, EntityKind::Class, &["Outer"], "class Outer {\n  helper() {\n    return load(1);\n  }\n}", "Outer");
  let helper = symbol(NESTED, EntityKind::Method, &["Outer", "helper"], "helper() {\n    return load(1);\n  }", "helper");
  let load = symbol(NESTED, EntityKind::Function, &["load"], "function load(n) { return n; }", "load");
  let call = reference(RefKind::Call, "load", span_of(NESTED, "load", 0));
  context(vec![("src/a.ts", file(NESTED, vec![outer, helper, load], vec![call]))])
}

fn nested_definitions() -> Vec<scip::types::Occurrence> {
  vec![
    definition("scip-typescript npm demo 1.0.0 src/a.ts`Outer#", vec![0, 6, 11]),
    definition("scip-typescript npm demo 1.0.0 src/a.ts`Outer#helper().", vec![1, 2, 8]),
    definition("scip-typescript npm demo 1.0.0 src/a.ts`load().", vec![5, 9, 13]),
  ]
}

fn entity_of(context: &ScipIngestContext, name: &str) -> atlas_core::EntityId {
  context.files.values().flat_map(|file| &file.symbols).find(|symbol| symbol.name == name).unwrap().entity
}

#[test]
fn should_map_a_reference_inside_a_nested_symbol_to_its_innermost_enclosing_symbol() {
  let context = nested_context();
  let mut occurrences = nested_definitions();
  occurrences.push(usage("scip-typescript npm demo 1.0.0 src/a.ts`load().", vec![2, 11, 15]));
  let bytes = index_bytes("scip-typescript", vec![document("src/a.ts", Utf16, occurrences)]);

  let facts = ingest(&bytes, &context).unwrap();

  let edges = &facts.per_file[&(repo(), rel("src/a.ts"))];
  assert_eq!(edges.len(), 1);
  let edge = &edges[0];
  assert_eq!(edge.kind, EdgeKind::Calls);
  assert_eq!(edge.from, entity_of(&context, "helper"));
  assert_eq!(edge.to, entity_of(&context, "load"));
  assert_eq!(edge.certainty, Certainty::CompilerVerified);
  assert_eq!(edge.evidence.len(), 1);
  assert_eq!(edge.evidence[0].span, span_of(NESTED, "load", 0));
  assert_eq!(edge.evidence[0].path, rel("src/a.ts"));
  assert_eq!(edge.evidence[0].content_hash, context.files[&rel("src/a.ts")].content_hash);
  assert_eq!(edge.evidence[0].producer.name, "scip:scip-typescript");
  assert_eq!((facts.stats.documents, facts.stats.definitions_mapped, facts.stats.references_mapped), (1, 3, 1));
  assert_eq!(
    (facts.stats.definitions_unmapped, facts.stats.references_external, facts.stats.references_unmapped),
    (0, 0, 0)
  );
  assert!(facts.unmapped_definitions.is_empty());
}

#[test]
fn should_derive_the_edge_kind_from_the_matching_reference_fact() {
  let source = "function make() { return new Widget(); }\nclass Widget {}\n";
  let make = symbol(source, EntityKind::Function, &["make"], "function make() { return new Widget(); }", "make");
  let widget = symbol(source, EntityKind::Class, &["Widget"], "class Widget {}", "Widget");
  let instantiation = reference(RefKind::Instantiation, "Widget", span_of(source, "Widget", 0));
  let context = context(vec![("src/a.ts", file(source, vec![make, widget], vec![instantiation]))]);
  let widget_symbol = "scip-typescript npm demo 1.0.0 src/a.ts`Widget#";
  let occurrences = vec![
    definition("scip-typescript npm demo 1.0.0 src/a.ts`make().", vec![0, 9, 13]),
    definition(widget_symbol, vec![1, 6, 12]),
    usage(widget_symbol, vec![0, 29, 35]),
    usage(widget_symbol, vec![0, 29, 35]),
  ];
  let facts = ingest(&index_bytes("scip-typescript", vec![document("src/a.ts", Utf16, occurrences)]), &context).unwrap();

  let edges = &facts.per_file[&(repo(), rel("src/a.ts"))];
  assert_eq!(edges.len(), 1);
  assert_eq!(edges[0].kind, EdgeKind::Instantiates);
  assert_eq!(edges[0].evidence.len(), 1);
}

#[test]
fn should_fall_back_to_references_when_no_reference_fact_matches() {
  let context = nested_context_without_facts();
  let mut occurrences = nested_definitions();
  occurrences.push(usage("scip-typescript npm demo 1.0.0 src/a.ts`load().", vec![2, 11, 15]));
  let facts = ingest(&index_bytes("scip-typescript", vec![document("src/a.ts", Utf16, occurrences)]), &context).unwrap();

  let edges = &facts.per_file[&(repo(), rel("src/a.ts"))];
  assert_eq!(edges.iter().map(|edge| edge.kind.clone()).collect::<Vec<_>>(), vec![EdgeKind::References]);
}

fn nested_context_without_facts() -> ScipIngestContext {
  let mut context = nested_context();
  context.files.values_mut().for_each(|file| file.references.clear());
  context
}

#[test]
fn should_merge_repeated_references_into_one_edge_with_each_evidence_span() {
  let source = "function a() { b(); b(); }\nfunction b() {}\n";
  let a = symbol(source, EntityKind::Function, &["a"], "function a() { b(); b(); }", "a");
  let b = symbol(source, EntityKind::Function, &["b"], "function b() {}", "b");
  let context = context(vec![("src/a.ts", file(source, vec![a, b], vec![]))]);
  let target = "scip-typescript npm demo 1.0.0 src/a.ts`b().";
  let occurrences = vec![
    definition("scip-typescript npm demo 1.0.0 src/a.ts`a().", vec![0, 9, 10]),
    definition(target, vec![1, 9, 10]),
    usage(target, vec![0, 15, 16]),
    usage(target, vec![0, 20, 21]),
  ];
  let facts = ingest(&index_bytes("scip-typescript", vec![document("src/a.ts", Utf16, occurrences)]), &context).unwrap();

  let edges = &facts.per_file[&(repo(), rel("src/a.ts"))];
  assert_eq!(edges.len(), 1);
  assert_eq!(edges[0].evidence.len(), 2);
  assert_eq!(facts.stats.references_mapped, 2);
}

fn non_ascii_context() -> (String, ScipIngestContext) {
  let source = "function café() {}\nfunction main() { const s = \"😀é\"; café(); }\n".to_owned();
  let cafe = symbol(&source, EntityKind::Function, &["café"], "function café() {}", "café");
  let main = symbol(&source, EntityKind::Function, &["main"], "function main() { const s = \"😀é\"; café(); }", "main");
  let call = reference(RefKind::Call, "café", span_of(&source, "café", 1));
  let context = context(vec![("src/a.ts", file(&source, vec![cafe, main], vec![call]))]);
  (source, context)
}

#[test]
fn should_convert_utf16_columns_on_non_ascii_lines_to_byte_spans() {
  let (source, context) = non_ascii_context();
  let cafe = "scip-typescript npm demo 1.0.0 src/a.ts`café().";
  let occurrences = vec![
    definition(cafe, vec![0, 9, 13]),
    definition("scip-typescript npm demo 1.0.0 src/a.ts`main().", vec![1, 9, 13]),
    usage(cafe, vec![1, 35, 39]),
  ];
  let facts = ingest(&index_bytes("scip-typescript", vec![document("src/a.ts", Utf16, occurrences)]), &context).unwrap();

  let edges = &facts.per_file[&(repo(), rel("src/a.ts"))];
  assert_eq!(edges.len(), 1);
  assert_eq!(edges[0].kind, EdgeKind::Calls);
  assert_eq!(edges[0].from, entity_of(&context, "main"));
  assert_eq!(edges[0].to, entity_of(&context, "café"));
  assert_eq!(edges[0].evidence[0].span, span_of(&source, "café", 1));
  assert_eq!(facts.stats.definitions_mapped, 2);
}

#[test]
fn should_convert_utf8_columns_as_byte_offsets() {
  let (source, context) = non_ascii_context();
  let cafe = "scip-typescript npm demo 1.0.0 src/a.ts`café().";
  let call_start = source.lines().nth(1).unwrap().find("café();").unwrap() as i32;
  let occurrences = vec![
    definition(cafe, vec![0, 9, 14]),
    definition("scip-typescript npm demo 1.0.0 src/a.ts`main().", vec![1, 9, 13]),
    usage(cafe, vec![1, call_start, call_start + 5]),
  ];
  let facts = ingest(&index_bytes("rust-analyzer", vec![document("src/a.ts", Utf8, occurrences)]), &context).unwrap();

  let edges = &facts.per_file[&(repo(), rel("src/a.ts"))];
  assert_eq!(edges.len(), 1);
  assert_eq!(edges[0].evidence[0].span, span_of(&source, "café", 1));
}

#[test]
fn should_not_misread_utf16_columns_as_utf8_when_the_encoding_is_declared() {
  let (_, context) = non_ascii_context();
  let cafe = "scip-typescript npm demo 1.0.0 src/a.ts`café().";
  let occurrences = vec![definition(cafe, vec![0, 9, 13]), usage(cafe, vec![1, 35, 39])];
  let utf8_reading = ingest(&index_bytes("x", vec![document("src/a.ts", Utf8, occurrences)]), &context).unwrap();

  assert!(utf8_reading.per_file[&(repo(), rel("src/a.ts"))].is_empty());
  assert_eq!(utf8_reading.stats.definitions_mapped, 0);
  assert_eq!(utf8_reading.stats.definitions_unmapped, 1);
}

#[test]
fn should_assume_the_tool_default_encoding_when_none_is_declared_and_warn() {
  let (_, context) = non_ascii_context();
  let cafe = "scip-typescript npm demo 1.0.0 src/a.ts`café().";
  let occurrences = vec![
    definition(cafe, vec![0, 9, 13]),
    definition("scip-typescript npm demo 1.0.0 src/a.ts`main().", vec![1, 9, 13]),
  ];
  let facts = ingest(
    &index_bytes("scip-typescript", vec![document("src/a.ts", UnspecifiedPositionEncoding, occurrences)]),
    &context,
  )
  .unwrap();

  assert_eq!(facts.stats.definitions_mapped, 2);
  assert_eq!(facts.warnings.len(), 1);
  assert!(facts.warnings[0].contains("does not declare a position encoding"));
}

#[test]
fn should_count_external_references_by_package_and_ignore_local_symbols() {
  let context = nested_context();
  let mut occurrences = nested_definitions();
  occurrences.push(usage("scip-typescript npm react 18.2.0 index.d.ts`useState().", vec![2, 11, 15]));
  occurrences.push(usage("scip-typescript npm react 18.2.0 index.d.ts`useState().", vec![2, 11, 15]));
  occurrences.push(usage("scip-typescript npm @types/node 20.0.0 fs.d.ts`readFile().", vec![2, 11, 15]));
  occurrences.push(usage("local 7", vec![2, 11, 15]));
  let facts = ingest(&index_bytes("scip-typescript", vec![document("src/a.ts", Utf16, occurrences)]), &context).unwrap();

  assert!(facts.per_file[&(repo(), rel("src/a.ts"))].is_empty());
  assert_eq!(facts.external_references.get("react"), Some(&2));
  assert_eq!(facts.external_references.get("@types/node"), Some(&1));
  assert_eq!(facts.stats.references_external, 3);
  assert_eq!((facts.stats.references_mapped, facts.stats.references_unmapped), (0, 0));
}

#[test]
fn should_count_a_reference_without_an_enclosing_symbol_as_unmapped() {
  let source = "const x = load();\nfunction load() {}\n";
  let load = symbol(source, EntityKind::Function, &["load"], "function load() {}", "load");
  let context = context(vec![("src/a.ts", file(source, vec![load], vec![]))]);
  let target = "scip-typescript npm demo 1.0.0 src/a.ts`load().";
  let occurrences = vec![definition(target, vec![1, 9, 13]), usage(target, vec![0, 10, 14])];
  let facts = ingest(&index_bytes("scip-typescript", vec![document("src/a.ts", Utf16, occurrences)]), &context).unwrap();

  assert_eq!(facts.stats.references_unmapped, 1);
  assert_eq!(facts.stats.references_mapped, 0);
}

#[test]
fn should_report_our_symbols_that_scip_never_defined() {
  let context = nested_context();
  let occurrences = vec![definition("scip-typescript npm demo 1.0.0 src/a.ts`Outer#", vec![0, 6, 11])];
  let facts = ingest(&index_bytes("scip-typescript", vec![document("src/a.ts", Utf16, occurrences)]), &context).unwrap();

  let names: Vec<&str> = facts.unmapped_definitions.iter().map(|unmapped| unmapped.name.as_str()).collect();
  assert_eq!(names, vec!["helper", "load"]);
  assert_eq!(facts.unmapped_definitions[0].path, rel("src/a.ts"));
}

#[test]
fn should_count_scip_definitions_that_match_none_of_our_symbols() {
  let context = nested_context();
  let occurrences = vec![definition("scip-typescript npm demo 1.0.0 src/a.ts`Outer#field.", vec![4, 0, 1])];
  let facts = ingest(&index_bytes("scip-typescript", vec![document("src/a.ts", Utf16, occurrences)]), &context).unwrap();

  assert_eq!((facts.stats.definitions_mapped, facts.stats.definitions_unmapped), (0, 1));
}

#[test]
fn should_not_map_a_symbol_defined_at_two_of_our_symbols() {
  let context = nested_context();
  let shared = "scip-typescript npm demo 1.0.0 src/a.ts`dup().";
  let mut occurrences = vec![definition(shared, vec![0, 6, 11]), definition(shared, vec![5, 9, 13])];
  occurrences.push(usage(shared, vec![2, 11, 15]));
  let facts = ingest(&index_bytes("scip-typescript", vec![document("src/a.ts", Utf16, occurrences)]), &context).unwrap();

  assert!(facts.per_file[&(repo(), rel("src/a.ts"))].is_empty());
  assert_eq!(facts.stats.definitions_ambiguous, 1);
  assert_eq!(facts.stats.references_unmapped, 1);
  assert!(facts.warnings.iter().any(|warning| warning.contains("defined at several of our symbols")));
}

#[test]
fn should_turn_implementation_relationships_into_implements_edges() {
  let source = "interface Animal {}\nclass Dog {}\n";
  let animal = symbol(source, EntityKind::Interface, &["Animal"], "interface Animal {}", "Animal");
  let dog = symbol(source, EntityKind::Class, &["Dog"], "class Dog {}", "Dog");
  let context = context(vec![("src/a.ts", file(source, vec![animal, dog], vec![]))]);
  let (animal_symbol, dog_symbol) = ("scip-typescript npm demo 1.0.0 src/a.ts`Animal#", "scip-typescript npm demo 1.0.0 src/a.ts`Dog#");
  let mut doc = document("src/a.ts", Utf16, vec![definition(animal_symbol, vec![0, 10, 16]), definition(dog_symbol, vec![1, 6, 9])]);
  doc.symbols.push(implementation(dog_symbol, animal_symbol));
  doc.symbols.push(implementation(dog_symbol, "scip-typescript npm other 1.0.0 lib.d.ts`Disposable#"));
  let facts = ingest(&index_bytes("scip-typescript", vec![doc]), &context).unwrap();

  let edges = &facts.per_file[&(repo(), rel("src/a.ts"))];
  assert_eq!(edges.len(), 1);
  assert_eq!(edges[0].kind, EdgeKind::Implements);
  assert_eq!(edges[0].from, entity_of(&context, "Dog"));
  assert_eq!(edges[0].to, entity_of(&context, "Animal"));
  assert_eq!(edges[0].certainty, Certainty::CompilerVerified);
  assert_eq!(edges[0].evidence[0].span, span_of(source, "Dog", 0));
  assert_eq!((facts.stats.relationships_mapped, facts.stats.relationships_unmapped), (1, 1));
}

#[test]
fn should_turn_class_to_class_implementation_relationships_into_extends_edges() {
  let source = "class Base {}\nclass Child {}\n";
  let base = symbol(source, EntityKind::Class, &["Base"], "class Base {}", "Base");
  let child = symbol(source, EntityKind::Class, &["Child"], "class Child {}", "Child");
  let context = context(vec![("src/a.ts", file(source, vec![base, child], vec![]))]);
  let (base_symbol, child_symbol) = ("scip-typescript npm demo 1.0.0 src/a.ts`Base#", "scip-typescript npm demo 1.0.0 src/a.ts`Child#");
  let mut doc = document("src/a.ts", Utf16, vec![definition(base_symbol, vec![0, 6, 10]), definition(child_symbol, vec![1, 6, 11])]);
  doc.symbols.push(implementation(child_symbol, base_symbol));
  let facts = ingest(&index_bytes("scip-typescript", vec![doc]), &context).unwrap();

  let edges = &facts.per_file[&(repo(), rel("src/a.ts"))];
  assert_eq!(edges.iter().map(|edge| edge.kind.clone()).collect::<Vec<_>>(), vec![EdgeKind::Extends]);
}

#[test]
fn should_resolve_document_paths_below_the_package_dir_and_skip_foreign_documents() {
  let source = "function a() {}\n";
  let a = symbol(source, EntityKind::Function, &["a"], "function a() {}", "a");
  let context = ScipIngestContext::new(repo())
    .in_package_dir(rel("packages/api"))
    .with_file(rel("packages/api/src/a.ts"), file(source, vec![a], vec![]));
  let occurrences = vec![definition("scip-typescript npm demo 1.0.0 src/a.ts`a().", vec![0, 9, 10])];
  let documents = vec![
    document("src/a.ts", Utf16, occurrences),
    document("../../elsewhere/b.ts", Utf16, vec![]),
    document("src/not-ours.ts", Utf16, vec![]),
  ];
  let facts = ingest(&index_bytes("scip-typescript", documents), &context).unwrap();

  assert!(facts.per_file.contains_key(&(repo(), rel("packages/api/src/a.ts"))));
  assert_eq!(facts.per_file.len(), 1);
  assert_eq!((facts.stats.documents, facts.stats.documents_outside_project), (1, 2));
  assert_eq!(facts.stats.definitions_mapped, 1);
}

#[test]
fn should_name_the_owner_pass_after_repo_and_path() {
  assert_eq!(atlas_scip::ScipFacts::owner_pass(&repo(), &rel("src/a.ts")), "scip:demo:src/a.ts");
}

#[test]
fn should_reject_bytes_that_are_not_a_scip_index() {
  let result = ingest(&[0xff, 0xff, 0xff, 0xff, 0xff], &ScipIngestContext::new(repo()));
  assert!(matches!(result, Err(ScipError::Decode(_))));
}

#[test]
fn should_skip_occurrences_with_malformed_ranges_without_failing() {
  let context = nested_context();
  let mut occurrences = nested_definitions();
  occurrences.push(usage("scip-typescript npm demo 1.0.0 src/a.ts`load().", vec![2, 11, 400]));
  let facts = ingest(&index_bytes("scip-typescript", vec![document("src/a.ts", Utf16, occurrences)]), &context).unwrap();

  assert_eq!(facts.stats.references_unmapped, 1);
  assert!(facts.per_file[&(repo(), rel("src/a.ts"))].is_empty());
  assert!(facts.warnings.iter().any(|warning| warning.contains("out-of-range")));
}

const TR: &str = "scip-rust npm demo 1.0.0 Tr#";
const OTHER_TR: &str = "scip-rust npm demo 1.0.0 other/Tr#";
const FOO_A: &str = "scip-rust npm demo 1.0.0 a/Foo#";
const FOO_B: &str = "scip-rust npm demo 1.0.0 b/Foo#";

fn impl_fixture(source: &str, impl_text: &str, self_name: &str, trait_name: &str) -> ScipIngestContext {
  let tr = symbol(source, EntityKind::Trait, &["Tr"], "trait Tr {}", "Tr");
  let other_tr = symbol_nth(source, EntityKind::Trait, &["other", "Tr"], "trait Tr {}", 1, "Tr");
  let foo_a = symbol(source, EntityKind::Struct, &["a", "Foo"], "struct Foo {}", "Foo");
  let foo_b = symbol_nth(source, EntityKind::Struct, &["b", "Foo"], "struct Foo {}", 1, "Foo");
  let file = file(source, vec![tr, other_tr, foo_a, foo_b], vec![]);
  context(vec![("src/lib.rs", with_trait_impl(file, source, impl_text, self_name, trait_name))])
}

fn impl_definitions() -> Vec<scip::types::Occurrence> {
  vec![
    definition(TR, vec![0, 6, 8]),
    definition(OTHER_TR, vec![1, 6, 8]),
    definition(FOO_A, vec![2, 7, 10]),
    definition(FOO_B, vec![3, 7, 10]),
  ]
}

fn impl_edges(context: &ScipIngestContext, mut occurrences: Vec<scip::types::Occurrence>) -> Vec<(atlas_core::EntityId, atlas_core::EntityId)> {
  let mut all = impl_definitions();
  all.append(&mut occurrences);
  let facts = ingest(&index_bytes("rust-analyzer", vec![document("src/lib.rs", Utf8, all)]), context).unwrap();
  facts.per_file[&(repo(), rel("src/lib.rs"))]
    .iter()
    .filter(|edge| edge.kind == EdgeKind::Implements)
    .map(|edge| (edge.from, edge.to))
    .collect()
}

fn lib_entity(context: &ScipIngestContext, item: &[&str]) -> atlas_core::EntityId {
  context.files[&rel("src/lib.rs")]
    .symbols
    .iter()
    .find(|symbol| symbol.key.item_path() == item)
    .unwrap()
    .entity
}

const HEADER_SOURCE: &str = "trait Tr {}\ntrait Tr {}\nstruct Foo {}\nstruct Foo {}\n";

#[test]
fn should_ignore_a_same_named_type_in_a_where_clause_when_deriving_impl_edges() {
  let line = "impl Tr for a::Foo where b::Foo: Other {}";
  let source = format!("{HEADER_SOURCE}{line}\n");
  let context = impl_fixture(&source, line, "Foo", "Tr");
  let edges = impl_edges(&context, vec![usage(TR, vec![4, 5, 7]), usage(FOO_A, vec![4, 15, 18]), usage(FOO_B, vec![4, 28, 31])]);

  assert_eq!(edges, vec![(lib_entity(&context, &["a", "Foo"]), lib_entity(&context, &["Tr"]))]);
}

#[test]
fn should_not_take_a_generic_argument_named_like_the_trait_as_the_trait() {
  let line = "impl Tr<other::Tr> for Foo {}";
  let source = format!("{HEADER_SOURCE}{line}\n");
  let context = impl_fixture(&source, line, "Foo", "Tr");
  let edges = impl_edges(&context, vec![usage(TR, vec![4, 5, 7]), usage(OTHER_TR, vec![4, 16, 18]), usage(FOO_A, vec![4, 23, 26])]);

  assert_eq!(edges, vec![(lib_entity(&context, &["a", "Foo"]), lib_entity(&context, &["Tr"]))]);
}

#[test]
fn should_derive_nothing_for_a_blanket_impl_or_an_impl_on_an_external_type() {
  let line = "impl<T> Tr for Vec<T> {}";
  let source = format!("{HEADER_SOURCE}{line}\n");
  let context = impl_fixture(&source, line, "Vec", "Tr");
  let external = "scip-rust cargo alloc 1.0.0 vec/Vec#";
  assert!(
    impl_edges(
      &context,
      vec![usage(TR, vec![4, 12, 14]), usage(external, vec![4, 15, 18]), usage("local 3", vec![4, 19, 20])]
    )
    .is_empty()
  );

  let blanket = "impl<T: Tr> Tr for T {}";
  let source = format!("{HEADER_SOURCE}{blanket}\n");
  let context = impl_fixture(&source, blanket, "T", "Tr");
  assert!(impl_edges(&context, vec![usage(TR, vec![4, 8, 10]), usage(TR, vec![4, 12, 14]), usage("local 3", vec![4, 18, 19])]).is_empty());
}

#[test]
fn should_derive_nothing_when_the_self_type_is_not_a_plain_path() {
  let line = "impl Tr for &Foo {}";
  let source = format!("{HEADER_SOURCE}{line}\n");
  let context = impl_fixture(&source, line, "Foo", "Tr");
  assert!(impl_edges(&context, vec![usage(TR, vec![4, 5, 7]), usage(FOO_A, vec![4, 13, 16])]).is_empty());
}
