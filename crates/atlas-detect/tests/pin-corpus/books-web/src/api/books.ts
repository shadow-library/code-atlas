import createClient from 'openapi-fetch';
import type { paths } from '@acme/books-api/openapi';

const books = createClient<paths>({ baseUrl: process.env.BOOKS_API_URL, headers: { Authorization: `Bearer ${sessionToken()}` } });

function sessionToken(): string {
  return 'token';
}

export async function loadBook(id: string) {
  return books.GET('/books/{id}', { params: { path: { id } } });
}

export async function addReview(bookId: string, body: { stars: number }) {
  return books.POST('/books/{bookId}/reviews', { params: { path: { bookId } }, body });
}

export async function searchBooks(term: string) {
  return books.GET(`/books/search/${term}`, { params: { query: { limit: 20 } } });
}

export async function removeShelf(shelfId: string) {
  return books.DELETE('/shelves/{shelfId}.json', { params: { path: { shelfId } } });
}
