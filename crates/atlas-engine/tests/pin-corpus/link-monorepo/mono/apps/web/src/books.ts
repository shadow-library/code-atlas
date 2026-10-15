import createClient from 'openapi-fetch';
import type { paths } from '@acme/books-api/openapi';

const books = createClient<paths>({ baseUrl: '/' });

export const loadBookItem = (id: string) => books.GET('/api/items/{id}', { params: { path: { id } } });

export const checkBooks = () => books.GET('/api/health');
