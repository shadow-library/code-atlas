import { Controller, Get, UseGuards } from '@nestjs/common';

import { SessionGuard } from '../auth/session.guard';

@Controller('admin')
@UseGuards(SessionGuard)
export class AdminController {
  @Get('stats')
  stats() {
    return { uptimeSeconds: Math.floor(process.uptime()), pid: process.pid };
  }
}
