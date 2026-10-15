import createClient from 'openapi-fetch';
import type { paths } from '../../../books-api/src/generated/openapi';

export const booksApi = createClient<paths>({ baseUrl: `${import.meta.env.VITE_BOOKS_URL}/api/` });

export const staticApi = createClient<paths>({ baseUrl: '/api/' });

const internalApi = createClient<paths>();

export function unused() {
  return internalApi;
}
