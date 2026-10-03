import { EventEmitter } from 'node:events';

import { formatMoney } from '../util/money';

export interface GlobexOrder {
  id: string;
  amount: number;
}

export const orderEvents = new EventEmitter();

export class OrdersService {
  private readonly orders = new Map<string, GlobexOrder>();

  findOne(id: string): (GlobexOrder & { label: string }) | null {
    const order = this.orders.get(id);
    return order ? { ...order, label: formatMoney(order.amount) } : null;
  }

  create(order: GlobexOrder): GlobexOrder {
    this.orders.set(order.id, order);
    orderEvents.emit('order.created', order);
    return order;
  }
}
