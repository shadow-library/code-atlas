import express from 'express';

import { apiRouter } from './routes';

const API_PREFIX = '/api';
const app = express();

app.use('/api', apiRouter);

app.get(`${API_PREFIX}/status`, (_req, res) => {
  res.json({ ok: true });
});

app.get('/legacy' + '/ping', (_req, res) => {
  res.send('pong');
});

app.get(buildPath('reports'), (_req, res) => {
  res.json([]);
});

function buildPath(name: string): string {
  return `/${name}/${new Date().getFullYear()}`;
}

app.listen(3000);
