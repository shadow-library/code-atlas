pub fn to_cents(amount: &str) -> i64 {
  let trimmed = amount.trim();
  let (negative, unsigned) = match trimmed.strip_prefix('-') {
    Some(rest) => (true, rest),
    None => (false, trimmed),
  };
  let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
  let fraction = format!("{fraction:0<2}");
  let cents = whole.parse::<i64>().unwrap_or(0) * 100 + fraction[..2].parse::<i64>().unwrap_or(0);
  if negative {
    -cents
  } else {
    cents
  }
}

#[cfg(test)]
mod tests {
  use super::to_cents;

  #[test]
  fn converts_whole_and_fractional_amounts() {
    assert_eq!(to_cents("12.34"), 1234);
    assert_eq!(to_cents("12.3"), 1230);
    assert_eq!(to_cents("12"), 1200);
  }

  #[test]
  fn keeps_the_sign_of_negative_amounts() {
    assert_eq!(to_cents("-1.50"), -150);
  }
}
