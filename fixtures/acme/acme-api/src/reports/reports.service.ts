import { Injectable } from '@nestjs/common';
import { InjectRepository } from '@nestjs/typeorm';
import { Repository } from 'typeorm';

import { roundHalfUp } from '../billing/rounding';
import { formatMoney } from '../common/money';
import { Order } from '../orders/order.entity';

@Injectable()
export class ReportsService {
  constructor(@InjectRepository(Order) private readonly orders: Repository<Order>) {}

  async exportCsv(): Promise<string> {
    const orders = await this.orders.find();
    const rows = orders.map((order) => `${order.id},${order.userId},${formatMoney(order.totalCents)}`);
    const average = orders.length ? roundHalfUp(orders.reduce((sum, order) => sum + order.totalCents, 0) / orders.length) : 0;
    return ['id,user,total', ...rows, `average,,${formatMoney(average)}`].join('\n');
  }
}
