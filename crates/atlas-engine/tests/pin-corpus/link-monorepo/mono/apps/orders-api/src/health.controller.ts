import { Controller, Get, UseGuards } from '@nestjs/common';

import { InternalGuard } from './internal.guard';

@Controller('health')
export class HealthController {
  @Get()
  @UseGuards(InternalGuard)
  check() {
    return 'ok';
  }
}
