import legacy from './api/legacy.js';
import { booksApi } from './vendor/client';

export const removeBook = (id: string) => legacy.delete(`/books/${id}`);

export const notDeclared = () => booksApi.get('/books/{id}');
