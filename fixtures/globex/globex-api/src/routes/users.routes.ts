import { type Express } from 'express';

import { requireGlobexKey } from '../middleware/auth';
import { UsersService } from '../services/users.service';

const usersService = new UsersService();

export function registerUserRoutes(app: Express): void {
  app.get('/users/:id', requireGlobexKey, async (req, res) => {
    const user = await usersService.findOne(req.params.id);
    if (!user) return res.status(404).json({ error: 'not found' });
    return res.json(user);
  });
}
