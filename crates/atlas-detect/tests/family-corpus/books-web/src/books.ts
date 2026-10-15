import { booksApi as api, formatTitle } from './api/client';
import * as clients from '@acme/books-client';
import { createBooksClient } from '@acme/books-client';

export const loadBook = (id: string) => api.get('/books/{id}', { headers: { Authorization: `Bearer ${id}` } });

export const addBook = (book: { title: string }) => clients.booksApi.post('/books', { ...book, title: formatTitle(book.title) });

export async function removeShelf(shelfId: string) {
  const client = await createBooksClient({ baseUrl: process.env.BOOKS_URL });
  return client.delete(`/shelves/${shelfId}`);
}

export function shadowed(api: { get(path: string): void }) {
  return api.get('/books/{id}');
}

export const unlisted = () => api.put('/books/{id}');
