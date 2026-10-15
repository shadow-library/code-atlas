import { CanActivate, Injectable } from '@nestjs/common';

@Injectable()
export class InternalGuard implements CanActivate {
  canActivate(): boolean {
    return true;
  }
}
