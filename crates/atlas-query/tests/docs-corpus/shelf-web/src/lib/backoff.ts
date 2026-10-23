import { createShelfClient } from './shelf-client';

export function backoffDelay(attempt: number): number {
  return Math.min(1000 * 2 ** attempt, 30000);
}

export function loadWithBackoff(id: string) {
  const client = createShelfClient({ baseUrl: '/api', retries: 3 });
  return client.fetchShelf(id);
}
