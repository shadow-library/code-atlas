import { waitBeforeRetry } from '../lib/backoff';

export interface Shelf {
  id: string;
  titles: string[];
}

export async function fetchShelf(id: string, attempt = 1): Promise<Shelf> {
  const response = await fetch(`/api/shelves/${id}`);
  if (response.status === 503 && attempt < 3) {
    await new Promise((resolve) => setTimeout(resolve, waitBeforeRetry(undefined, attempt)));
    return fetchShelf(id, attempt + 1);
  }
  return (await response.json()) as Shelf;
}
