import { type Express } from 'express';

import { requireGlobexKey } from '../middleware/auth';
import { OrdersService } from '../services/orders.service';

const ordersService = new OrdersService();

export function registerOrderRoutes(app: Express): void {
  app.get('/orders/:id', requireGlobexKey, (req, res) => {
    const order = ordersService.findOne(req.params.id);
    if (!order) return res.status(404).json({ error: 'not found' });
    return res.json(order);
  });

  app.post('/orders', requireGlobexKey, (req, res) => {
    return res.status(201).json(ordersService.create(req.body));
  });
}
