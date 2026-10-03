use rusqlite::Connection;

#[test]
fn should_open_the_bundled_sqlite_with_json_support() {
  let connection = Connection::open_in_memory().expect("in-memory database must open");
  let value: i64 = connection
    .query_row("SELECT json_extract('{\"a\": 7}', '$.a')", [], |row| row.get(0))
    .expect("json1 must be available");
  assert_eq!(value, 7);
}
