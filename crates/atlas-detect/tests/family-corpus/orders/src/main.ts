import { AcmeServer } from '@acme/http';

class AppModule {}

async function boot() {
  const app = await AcmeServer.create(AppModule);
  app.setGlobalPrefix('api/v2');
  await app.listen(3000);
}

boot();
