use protobuf::Message;
use scip::types::{Document, Index};

#[test]
fn should_round_trip_a_scip_index_through_protobuf() {
  let mut index = Index::new();
  let mut document = Document::new();
  document.relative_path = "src/lib.rs".to_owned();
  index.documents.push(document);
  let bytes = index.write_to_bytes().expect("index must serialise");
  let decoded = Index::parse_from_bytes(&bytes).expect("index must parse");
  assert_eq!(decoded.documents.len(), 1);
  assert_eq!(decoded.documents[0].relative_path, "src/lib.rs");
}
