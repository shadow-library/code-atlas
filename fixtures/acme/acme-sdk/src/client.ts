import { type AcmeAuth, type AcmeClientOptions, type CreateOrderInput, type Order, type User } from './types';

export class AcmeClient {
  private readonly baseUrl: string;
  private readonly auth: AcmeAuth;

  constructor(options: AcmeClientOptions) {
    this.baseUrl = options.baseUrl.replace(/\/+$/, '');
    this.auth = options.auth;
  }

  private headers(): Record<string, string> {
    const headers: Record<string, string> = { 'Content-Type': 'application/json' };
    if (this.auth.type === 'bearer') headers['Authorization'] = `Bearer ${this.auth.token}`;
    else headers['x-api-key'] = this.auth.key;
    return headers;
  }

  async getUser(id: string): Promise<User> {
    const response = await fetch(`${this.baseUrl}/users/${id}`, { headers: this.headers() });
    return this.parse<User>(response);
  }

  async updateUser(id: string, patch: Partial<Pick<User, 'email' | 'displayName'>>): Promise<User> {
    const response = await fetch(`${this.baseUrl}/users/${id}`, { method: 'PATCH', headers: this.headers(), body: JSON.stringify(patch) });
    return this.parse<User>(response);
  }

  async createOrder(input: CreateOrderInput): Promise<Order> {
    const response = await fetch(`${this.baseUrl}/orders`, { method: 'POST', headers: this.headers(), body: JSON.stringify(input) });
    return this.parse<Order>(response);
  }

  async getOrder(id: string): Promise<Order> {
    const response = await fetch(`${this.baseUrl}/orders/${id}`, { headers: this.headers() });
    return this.parse<Order>(response);
  }

  private async parse<T>(response: Response): Promise<T> {
    if (!response.ok) throw new Error(`Acme API responded with ${response.status}`);
    return (await response.json()) as T;
  }
}
