import { Api, Guarded, Read, Write } from '@acme/http';

import { SessionGuard } from './session.guard';

@Api('orders')
@Guarded(SessionGuard)
export class OrdersController {
  @Read(':id')
  find() {
    return null;
  }

  @Write()
  create() {
    return null;
  }
}
