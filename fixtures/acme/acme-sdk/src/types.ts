export type AcmeAuth = { type: 'bearer'; token: string } | { type: 'apiKey'; key: string };

export interface AcmeClientOptions {
  baseUrl: string;
  auth: AcmeAuth;
}

export interface User {
  id: string;
  email: string;
  displayName: string;
}

export interface Order {
  id: string;
  userId: string;
  totalCents: number;
}

export interface CreateOrderInput {
  userId: string;
  subtotalCents: number;
}
