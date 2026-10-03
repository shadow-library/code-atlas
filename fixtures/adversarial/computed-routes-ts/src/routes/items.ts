import { Router } from 'express';

export const itemsRouter = Router();

itemsRouter.get('/:id', (req, res) => {
  res.json({ id: req.params.id });
});

itemsRouter.post('/', (req, res) => {
  res.status(201).json(req.body);
});
