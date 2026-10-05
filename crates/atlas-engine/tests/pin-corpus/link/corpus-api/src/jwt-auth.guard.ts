import { type CanActivate, type ExecutionContext, Injectable, UnauthorizedException } from '@nestjs/common';
import { JwtService } from '@nestjs/jwt';

@Injectable()
export class JwtAuthGuard implements CanActivate {
  constructor(private readonly jwt: JwtService) {}

  async canActivate(context: ExecutionContext): Promise<boolean> {
    const request = context.switchToHttp().getRequest();
    const header: string | undefined = request.headers['authorization'];
    if (!header?.startsWith('Bearer ')) throw new UnauthorizedException('Missing bearer token');
    try {
      request.user = await this.jwt.verifyAsync(header.slice('Bearer '.length), { secret: process.env.JWT_SECRET });
      return true;
    } catch {
      throw new UnauthorizedException('Invalid bearer token');
    }
  }
}
