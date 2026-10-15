import { AcmeServer } from '@acme/http';

class AcmeModule {}

async function boot(): Promise<void> {
  const app = await AcmeServer.create(AcmeModule);
  app.setGlobalPrefix('api');
}

boot();
