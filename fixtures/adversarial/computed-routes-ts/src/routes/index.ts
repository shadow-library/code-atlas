import { Router } from 'express';

import { itemsRouter } from './items';

export const apiRouter = Router();

apiRouter.use('/items', itemsRouter);
