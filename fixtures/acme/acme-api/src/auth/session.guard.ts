import { type CanActivate, type ExecutionContext, Injectable, UnauthorizedException } from '@nestjs/common';

import { SessionStore } from './session.store';

@Injectable()
export class SessionGuard implements CanActivate {
  constructor(private readonly sessions: SessionStore) {}

  async canActivate(context: ExecutionContext): Promise<boolean> {
    const request = context.switchToHttp().getRequest();
    const sessionId: string | undefined = request.cookies?.['acme_session'];
    if (!sessionId) throw new UnauthorizedException('Missing session cookie');
    const session = await this.sessions.find(sessionId);
    if (!session) throw new UnauthorizedException('Unknown session');
    request.user = session.user;
    return true;
  }
}
