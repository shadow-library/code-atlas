mod common;

use atlas_edit::{PlanViolation, TextEdit, file_diff};
use common::target;

fn render(original: &str, edits: &[TextEdit]) -> String {
  file_diff(&target("api", "a.ts"), original, edits, &[]).unwrap().to_string()
}

const HEADER: &str = "--- api:a.ts\n+++ api:a.ts\n";

#[test]
fn should_show_an_insertion_as_added_lines_between_context() {
  assert_eq!(render("a\nb\nc\n", &[TextEdit::insert(2, "x\n")]), format!("{HEADER}@@ -1,3 +1,4 @@\n a\n+x\n b\n c\n"));
}

#[test]
fn should_render_a_created_and_an_emptied_file() {
  assert_eq!(render("", &[TextEdit::insert(0, "a\nb\n")]), format!("{HEADER}@@ -0,0 +1,2 @@\n+a\n+b\n"));
  assert_eq!(render("a\n", &[TextEdit::delete(0..2)]), format!("{HEADER}@@ -1,1 +0,0 @@\n-a\n"));
}

#[test]
fn should_split_distant_edits_into_hunks_with_shifted_new_line_numbers() {
  let original = (1..=20).map(|line| format!("l{line}\n")).collect::<String>();
  let second = original.find("l2\n").unwrap();
  let eighteenth = original.find("l18\n").unwrap();
  let diff = render(
    &original,
    &[TextEdit::replace(second..second + 2, "two\ntwo again"), TextEdit::delete(eighteenth..eighteenth + 4)],
  );
  assert_eq!(
    diff,
    format!("{HEADER}@@ -1,5 +1,6 @@\n l1\n-l2\n+two\n+two again\n l3\n l4\n l5\n@@ -15,6 +16,5 @@\n l15\n l16\n l17\n-l18\n l19\n l20\n")
  );
}

#[test]
fn should_merge_edits_whose_context_overlaps() {
  let original = (1..=9).map(|line| format!("l{line}\n")).collect::<String>();
  let edits = [
    TextEdit::replace(0..2, "L1"),
    TextEdit::replace(original.find("l8").unwrap()..original.find("l8").unwrap() + 2, "L8"),
  ];
  assert_eq!(
    render(&original, &edits),
    format!("{HEADER}@@ -1,9 +1,9 @@\n-l1\n+L1\n l2\n l3\n l4\n l5\n l6\n l7\n-l8\n+L8\n l9\n")
  );
}

#[test]
fn should_strip_carriage_returns_from_rendered_lines() {
  assert_eq!(render("a\r\nb\r\n", &[TextEdit::replace(3..4, "B")]), format!("{HEADER}@@ -1,2 +1,2 @@\n a\n-b\n+B\n"));
}

#[test]
fn should_reject_unsorted_or_out_of_bounds_edits() {
  let file = target("api", "a.ts");
  assert_eq!(
    file_diff(&file, "abc", &[TextEdit::replace(2..3, "x"), TextEdit::replace(0..1, "y")], &[]).unwrap_err(),
    PlanViolation::Unsorted { file: file.clone(), index: 1 }
  );
  assert_eq!(
    file_diff(&file, "abc", &[TextEdit::replace(0..9, "x")], &[]).unwrap_err(),
    PlanViolation::OutOfBounds {
      file: file.clone(),
      start: 0,
      end: 9,
      len: 3,
    }
  );
}
