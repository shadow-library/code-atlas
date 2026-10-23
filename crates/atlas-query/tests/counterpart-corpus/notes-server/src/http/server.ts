import { HANDLERS } from './routes';
import { PORT } from '../config';

export function describeServer(): string {
  return `listening on ${PORT} with ${Object.keys(HANDLERS).length} handlers`;
}
