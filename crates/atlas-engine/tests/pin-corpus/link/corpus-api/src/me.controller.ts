import { Controller, Get, UseGuards } from '@nestjs/common';

import { JwtAuthGuard } from './jwt-auth.guard';

@Controller('users')
export class MeController {
  @Get('me')
  @UseGuards(JwtAuthGuard)
  me() {
    return { id: 'me' };
  }
}
