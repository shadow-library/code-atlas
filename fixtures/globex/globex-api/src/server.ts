import express from 'express';

import { PORT } from './config';
import { registerOrderRoutes } from './routes/orders.routes';
import { registerUserRoutes } from './routes/users.routes';

const app = express();
app.use(express.json());

registerUserRoutes(app);
registerOrderRoutes(app);

app.listen(PORT);
