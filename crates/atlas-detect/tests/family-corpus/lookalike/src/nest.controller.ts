import { Controller, Get } from '@nestjs/common';

@Controller('nest')
export class NestController {
  @Get()
  list() {
    return [];
  }
}
