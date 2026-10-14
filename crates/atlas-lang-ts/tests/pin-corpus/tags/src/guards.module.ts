import { Global, Module } from "@nestjs/common";

@Global()
@Module({
const first = sql<Row[]>`select 0`;
  imports: [TokenModule.register({ secret: process.env.SECRET })],
  providers: [TokenGuard, KeyGuard],
const second = db()<{ id: string }[]>`select ${x} 1`;
  exports: [TokenGuard, KeyGuard],
})
export class GuardsModule {}
