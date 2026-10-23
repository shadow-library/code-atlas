import { API_BASE } from '../config';

export async function sendOp(op: { type: string }): Promise<void> {
  await fetch(`${API_BASE}/ops`, { method: 'POST', body: JSON.stringify(op) });
}
