import { Body, Controller, Get, Param, Post } from '@nestjs/common';

@Controller('items')
export class OrderItemsController {
  @Get(':itemId')
  findOne(@Param('itemId') itemId: string) {
    return { itemId };
  }

  @Post()
  create(@Body() item: { sku: string }) {
    return item;
  }
}
