import { Pool } from 'pg';

import { DATABASE_URL } from '../config';

export interface GlobexUser {
  id: string;
  name: string;
}

export class UsersService {
  private readonly pool = new Pool({ connectionString: DATABASE_URL });

  async findOne(id: string): Promise<GlobexUser | null> {
    const { rows } = await this.pool.query<GlobexUser>('SELECT id, name FROM users WHERE id = $1', [id]);
    return rows[0] ?? null;
  }
}
