import { fetchShelf } from './client';

test('fetches a shelf', async () => {
  await fetchShelf('shelf-1');
});
