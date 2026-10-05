import { type CanActivate, type ExecutionContext, Injectable, UnauthorizedException } from '@nestjs/common';
import { ConfigService } from '@nestjs/config';

@Injectable()
export class ApiKeyGuard implements CanActivate {
  constructor(private readonly configService: ConfigService) {}

  canActivate(context: ExecutionContext): boolean {
    const request = context.switchToHttp().getRequest();
    const provided: string | undefined = request.headers['x-api-key'];
    const allowed = (this.configService.get<string>('API_KEYS') ?? '').split(',').filter(Boolean);
    if (!provided || !allowed.includes(provided)) throw new UnauthorizedException('Invalid API key');
    request.user = { id: 'service', permissions: ['orders:read'] };
    return true;
  }
}
