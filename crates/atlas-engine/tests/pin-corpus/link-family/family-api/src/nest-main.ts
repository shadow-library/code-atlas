import { NestFactory } from '@nestjs/core';

class NestModule {}

async function bootstrap(): Promise<void> {
  const app = await NestFactory.create(NestModule);
  app.setGlobalPrefix('api');
}

bootstrap();
