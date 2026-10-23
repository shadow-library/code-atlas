import { type Shelf, fetchShelf } from '../api/client';

const cache = new Map<string, Shelf>();

export async function loadShelf(id: string): Promise<Shelf> {
  const known = cache.get(id);
  if (known) {
    return known;
  }
  const shelf = await fetchShelf(id);
  cache.set(id, shelf);
  return shelf;
}
