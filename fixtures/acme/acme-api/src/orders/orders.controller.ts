import { Body, Controller, Get, Param, Post, UseGuards } from '@nestjs/common';

import { JwtAuthGuard } from '../auth/jwt-auth.guard';
import { JwtOrApiKeyGuard } from '../auth/jwt-or-api-key.guard';
import { type CreateOrderInput, OrdersService } from './orders.service';

const ORDERS_PATH = 'orders';

@Controller(ORDERS_PATH)
export class OrdersController {
  constructor(private readonly ordersService: OrdersService) {}

  @Get(':id')
  @UseGuards(JwtOrApiKeyGuard)
  findOne(@Param('id') id: string) {
    return this.ordersService.findOne(id);
  }

  @Post()
  @UseGuards(JwtAuthGuard)
  create(@Body() input: CreateOrderInput) {
    return this.ordersService.create(input);
  }
}
