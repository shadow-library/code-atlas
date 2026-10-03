import { Global, Module } from '@nestjs/common';
import { JwtModule } from '@nestjs/jwt';

import { ApiKeyGuard } from './api-key.guard';
import { JwtAuthGuard } from './jwt-auth.guard';
import { JwtOrApiKeyGuard } from './jwt-or-api-key.guard';
import { PermissionsGuard } from './permissions.guard';
import { SessionGuard } from './session.guard';
import { SessionStore } from './session.store';

@Global()
@Module({
  imports: [JwtModule.register({ secret: process.env.JWT_SECRET })],
  providers: [JwtAuthGuard, ApiKeyGuard, SessionGuard, JwtOrApiKeyGuard, PermissionsGuard, SessionStore],
  exports: [JwtAuthGuard, ApiKeyGuard, SessionGuard, JwtOrApiKeyGuard, PermissionsGuard, SessionStore, JwtModule],
})
export class AuthModule {}
