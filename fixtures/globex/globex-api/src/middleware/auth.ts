import { type NextFunction, type Request, type Response } from 'express';

export function requireGlobexKey(req: Request, res: Response, next: NextFunction): void {
  if (req.header('x-globex-key') !== process.env.GLOBEX_KEY) {
    res.status(401).json({ error: 'unauthorized' });
    return;
  }
  next();
}
