import { Api, Auth, Read, Write } from '@acme/blend';

import { REPORT_OPTIONS } from './options';

const SYNC = 'reports:sync';
const shared = { service: 'reports' };

@Api('billing')
@Auth({ public: true })
export class BillingController {
  @Read('invoices')
  @Auth({ permission: 'billing:read' })
  invoices() {
    return [];
  }

  @Read('closed')
  @Auth({ public: false, permission: 'billing:admin' })
  closed() {
    return null;
  }

  @Read('shared')
  @Auth({ ...shared })
  shared() {
    return null;
  }

  @Read('plain')
  plain() {
    return null;
  }
}

@Api('reports')
@Auth({ permission: ['reports:read'] })
export class ReportsController {
  @Read()
  @Auth({ permission: ['reports:list'] })
  list() {
    return [];
  }

  @Read('export')
  @Auth({ permission: 'reports:export', service: 'exporter' })
  exportAll() {
    return null;
  }

  @Write('sync')
  @Auth({ bot: SYNC })
  sync() {
    return null;
  }

  @Read('both')
  @Auth({ service: 'exporter', bot: SYNC })
  both() {
    return null;
  }

  @Read('bound')
  @Auth(REPORT_OPTIONS)
  bound() {
    return null;
  }

  @Read('off')
  @Auth({ service: undefined })
  off() {
    return null;
  }

  @Read('mixed')
  @Auth({ service: 'exporter', session: 'reports' })
  mixed() {
    return null;
  }

  @Read('zero')
  @Auth({ service: 0 })
  zero() {
    return null;
  }

  @Read('inherited')
  inherited() {
    return null;
  }
}
