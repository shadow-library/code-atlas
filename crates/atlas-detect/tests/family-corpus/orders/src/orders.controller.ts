import { Api, Guarded, Header, PathParam, Payload, Read, Remove, Write } from '@acme/http';
import { AdminGuard, SessionGuard } from './guards';
import { RequirePermission } from './permissions';

const BASE = 'orders';

interface CreateOrder {
  sku: string;
}

@Api({ path: BASE })
@Guarded(SessionGuard)
export class OrdersController {
  @Read()
  list() {
    return [];
  }

  @Read(':id')
  get(@PathParam('id') id: string, @Header('x-tenant') tenant: string) {
    return id + tenant;
  }

  @Write()
  @RequirePermission('orders:write')
  create(@Payload() body: CreateOrder) {
    return body;
  }

  @Remove(['/:id', '/:id/purge'])
  @Guarded(new AdminGuard())
  remove(@PathParam('id') id: string) {
    return id;
  }

  @Write(`items/${BASE.length}`)
  dynamic() {
    return null;
  }
}
