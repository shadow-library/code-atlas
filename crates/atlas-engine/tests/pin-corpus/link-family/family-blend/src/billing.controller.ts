import { Api, Auth, Read } from '@acme/blend';

const SHARED = { service: 'billing' };

@Api('billing')
@Auth({ public: true })
export class BillingController {
  @Read('invoices')
  @Auth({ permission: 'billing:read' })
  invoices() {
    return [];
  }

  @Read('ledger')
  @Auth({ public: false, service: 'ledger' })
  ledger() {
    return null;
  }

  @Read('session')
  @Auth({ public: false })
  session() {
    return null;
  }

  @Read('shared')
  @Auth({ ...SHARED })
  shared() {
    return null;
  }
}
