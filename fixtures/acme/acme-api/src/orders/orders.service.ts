import { Injectable, NotFoundException } from '@nestjs/common';
import { EventEmitter2 } from '@nestjs/event-emitter';
import { InjectRepository } from '@nestjs/typeorm';
import { Repository } from 'typeorm';

import { BillingService } from '../billing/billing.service';
import { formatMoney } from '../common/money';
import { Order } from './order.entity';

export interface CreateOrderInput {
  userId: string;
  subtotalCents: number;
}

@Injectable()
export class OrdersService {
  constructor(
    @InjectRepository(Order) private readonly orders: Repository<Order>,
    private readonly events: EventEmitter2,
    private readonly billing: BillingService,
  ) {}

  async findOne(id: string) {
    const order = await this.orders.findOneBy({ id });
    if (!order) throw new NotFoundException(`Order ${id} not found`);
    return { ...order, display: formatMoney(order.totalCents) };
  }

  async create(input: CreateOrderInput): Promise<Order> {
    const totalCents = this.billing.applyTax(input.subtotalCents);
    const order = await this.orders.save(this.orders.create({ userId: input.userId, totalCents }));
    const payload = { orderId: order.id, userId: order.userId, totalCents };
    this.events.emit('order.created', payload);
    return order;
  }
}
