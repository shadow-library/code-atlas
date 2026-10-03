use serde::Deserialize;

use crate::money::to_cents;
use crate::store::{Invoice, InvoiceStore};

pub struct Message {
  pub topic: String,
  pub payload: String,
}

pub trait MessageQueue {
  fn subscribe(&mut self, topic: &str) -> Vec<Message>;
}

#[derive(Deserialize)]
struct OrderCreated {
  #[serde(rename = "orderId")]
  order_id: String,
  #[serde(rename = "totalCents")]
  total_cents: i64,
}

pub fn consume_order_created(queue: &mut dyn MessageQueue, store: &dyn InvoiceStore) -> usize {
  let messages = queue.subscribe("order.created");
  let mut created = 0;
  for message in messages {
    let Ok(event) = serde_json::from_str::<OrderCreated>(&message.payload) else { continue };
    let amount_cents = to_cents(&format!("{}.{:02}", event.total_cents / 100, event.total_cents % 100));
    let invoice = Invoice { id: format!("inv_{}", event.order_id), order_id: event.order_id, amount_cents };
    if store.insert(invoice).is_ok() {
      created += 1;
    }
  }
  created
}
