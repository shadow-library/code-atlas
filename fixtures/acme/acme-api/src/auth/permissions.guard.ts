import { type CanActivate, type ExecutionContext, ForbiddenException, Injectable } from '@nestjs/common';
import { Reflector } from '@nestjs/core';

import { PERMISSION_KEY } from './require-permission.decorator';

@Injectable()
export class PermissionsGuard implements CanActivate {
  constructor(private readonly reflector: Reflector) {}

  canActivate(context: ExecutionContext): boolean {
    const required = this.reflector.getAllAndOverride<string | undefined>(PERMISSION_KEY, [context.getHandler(), context.getClass()]);
    if (!required) return true;
    const user = context.switchToHttp().getRequest().user as { permissions?: string[] } | undefined;
    if (!user?.permissions?.includes(required)) throw new ForbiddenException(`Missing permission ${required}`);
    return true;
  }
}
