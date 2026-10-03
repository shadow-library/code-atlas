use serde::Deserialize;

use crate::money::to_cents;

#[derive(Deserialize)]
struct OrderCreated {
  id: String,
  amount: f64,
}

pub trait Bus {
  fn subscribe(&mut self, topic: &str) -> Vec<String>;
}

pub fn handle_order_created(bus: &mut dyn Bus) -> Vec<(String, i64)> {
  bus
    .subscribe("order.created")
    .iter()
    .filter_map(|raw| serde_json::from_str::<OrderCreated>(raw).ok())
    .map(|event| (event.id, to_cents(event.amount)))
    .collect()
}
