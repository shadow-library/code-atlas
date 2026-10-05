import { Controller, Get, Param, UseGuards } from '@nestjs/common';

import { ApiKeyGuard } from './api-key.guard';

@Controller('orders')
export class OrdersController {
  @Get(':id')
  @UseGuards(ApiKeyGuard)
  findOne(@Param('id') id: string) {
    return { id };
  }
}
