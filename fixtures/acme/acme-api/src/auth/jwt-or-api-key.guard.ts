import { type CanActivate, type ExecutionContext, Injectable, UnauthorizedException } from '@nestjs/common';

import { ApiKeyGuard } from './api-key.guard';
import { JwtAuthGuard } from './jwt-auth.guard';

@Injectable()
export class JwtOrApiKeyGuard implements CanActivate {
  constructor(
    private readonly jwtGuard: JwtAuthGuard,
    private readonly apiKeyGuard: ApiKeyGuard,
  ) {}

  async canActivate(context: ExecutionContext): Promise<boolean> {
    const request = context.switchToHttp().getRequest();
    if (request.headers['x-api-key']) return this.apiKeyGuard.canActivate(context);
    if (request.headers['authorization']) return this.jwtGuard.canActivate(context);
    throw new UnauthorizedException('No credentials supplied');
  }
}
