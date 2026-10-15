import makeClient from 'openapi-fetch';
import type * as BooksApi from '../../../books-api/src/generated/openapi';

export function loadAuthor(authorId: string) {
  const api = makeClient<BooksApi.paths>({ baseUrl: 'https://books.acme.test/v1' });
  return api.GET('/authors/{authorId}', { params: { path: { authorId } } });
}

export function notAClient(cache: Map<string, string>) {
  return cache.get('/authors/{authorId}');
}
