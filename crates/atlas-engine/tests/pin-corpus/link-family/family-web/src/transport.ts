import { createKitClient } from '@acme/http-kit';

export const kitClient = createKitClient({ surfaces: { orders: '/api/orders', exports: '/api/exports/' } });

export const ExportsRequest = kitClient.exports;
