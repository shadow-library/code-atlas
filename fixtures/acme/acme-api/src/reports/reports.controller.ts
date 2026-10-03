import { Controller, Get, UseGuards } from '@nestjs/common';

import { ApiKeyGuard } from '../auth/api-key.guard';
import { ReportsService } from './reports.service';

@Controller()
export class ReportsController {
  constructor(private readonly reportsService: ReportsService) {}

  @Get('v1/reports/export')
  @UseGuards(ApiKeyGuard)
  export() {
    return this.reportsService.exportCsv();
  }
}
