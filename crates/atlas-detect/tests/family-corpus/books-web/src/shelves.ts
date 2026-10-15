import { ShelfRequest, kitClient } from './transport';
import * as transport from './transport';

export const listShelves = (page: number) => ShelfRequest.get('/shelves').query({ page }).execute<string[]>();

export const renameShelf = (shelfId: string, name: string) => ShelfRequest.patch(`/shelves/${shelfId}`).body({ name }).execute();

export const currentSession = () => kitClient.session.get('/current');

export const endSession = () => transport.kitClient.session.delete('/current');

export const cachedShelves = () => ShelfRequest.cache.get('/shelves');
