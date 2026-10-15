import { Api } from '@acme/http';
import { OtherServer } from '@other/http';

async function boot() {
  const app = await OtherServer.create(Api);
  app.setGlobalPrefix('ignored');
}

boot();
