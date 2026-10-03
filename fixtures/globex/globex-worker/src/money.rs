pub fn to_cents(amount: f64) -> i64 {
  (amount * 100.0).round() as i64
}
