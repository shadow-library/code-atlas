export interface ShelfOptions {
  baseUrl: string;
  retries: number;
}

export function createShelfClient(options: ShelfOptions) {
  return new ShelfClient(options);
}

export class ShelfClient {
  constructor(private readonly options: ShelfOptions) {}

  async fetchShelf(id: string): Promise<string> {
    return `${this.options.baseUrl}/shelves/${id}`;
  }
}
