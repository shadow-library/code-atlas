import { createKitClient } from '@acme/http-kit';

const SURFACES = { shelves: '/api/v2/', session: `${process.env.SESSION_ORIGIN}/session` } as const;

export const kitClient = createKitClient({ surfaces: SURFACES, retries: 2 });

export const ShelfRequest = kitClient.shelves;

export const pingShelves = () => ShelfRequest.head('/ping').execute();
