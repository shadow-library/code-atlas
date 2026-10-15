export async function loadOrder(id: string) {
  return fetch(`${process.env.FAMILY_API_URL}/api/orders/${id}`);
}

export async function createOrder() {
  return fetch(`${process.env.FAMILY_API_URL}/api/orders`, { method: 'POST' });
}

export async function checkHealth() {
  return fetch(`${process.env.FAMILY_API_URL}/api/health`);
}
