import { Injectable } from '@nestjs/common';

export interface Session {
  id: string;
  user: { id: string; permissions: string[] };
}

@Injectable()
export class SessionStore {
  private readonly sessions = new Map<string, Session>();

  async find(id: string): Promise<Session | undefined> {
    return this.sessions.get(id);
  }

  async save(session: Session): Promise<void> {
    this.sessions.set(session.id, session);
  }
}
