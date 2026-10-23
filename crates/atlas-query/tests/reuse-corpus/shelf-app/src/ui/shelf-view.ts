import { loadShelf } from '../state/shelf-store';

export async function renderShelf(id: string): Promise<string> {
  const shelf = await loadShelf(id);
  return shelf.titles.join(', ');
}
