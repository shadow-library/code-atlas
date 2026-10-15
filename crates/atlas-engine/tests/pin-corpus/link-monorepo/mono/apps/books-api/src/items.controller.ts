import { Controller, Get, Param } from '@nestjs/common';

@Controller('items')
export class BookItemsController {
  @Get(':id')
  findOne(@Param('id') id: string) {
    return { id };
  }
}
