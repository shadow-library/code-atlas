mod consumer;
mod money;

use consumer::{handle_order_created, Bus};

struct StdinBus;

impl Bus for StdinBus {
  fn subscribe(&mut self, _topic: &str) -> Vec<String> {
    std::io::stdin().lines().map_while(Result::ok).collect()
  }
}

fn main() {
  let database_url = std::env::var("DATABASE_URL").unwrap_or_default();
  let totals = handle_order_created(&mut StdinBus);
  println!("{} orders processed against {}", totals.len(), database_url);
}
