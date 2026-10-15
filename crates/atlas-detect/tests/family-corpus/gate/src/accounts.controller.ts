import { Api, Auth, Authenticated, OptionalAuth, Public, Read, RequireScope, Session, Write } from '@acme/gate';
import { Public as ElsewherePublic } from '@acme/other';

import { BOT_SYNC, PROJECTS_READ } from './permissions';

const AUDIT = 'accounts:audit';
const defaults = { public: false };
const overrides = { permission: 'accounts:admin' };

@Api('accounts')
@Authenticated()
export class AccountsController {
  @Read()
  list() {
    return [];
  }

  @Read('health')
  @Public()
  health() {
    return 'ok';
  }

  @Read('me')
  @OptionalAuth()
  @Public()
  me() {
    return null;
  }

  @Read('preview')
  @OptionalAuth()
  preview() {
    return null;
  }

  @Read('open')
  @Auth({ public: true })
  open() {
    return null;
  }

  @Read('guest')
  @Auth({ optional: true, permission: PROJECTS_READ })
  guest() {
    return null;
  }

  @Read('closed')
  @Auth({ public: false })
  closed() {
    return null;
  }

  @Read('spread')
  @Auth({ public: true, ...defaults })
  spread() {
    return null;
  }

  @Read('lookalike')
  @ElsewherePublic()
  lookalike() {
    return null;
  }

  @Write('sync')
  @Auth({ bot: BOT_SYNC, permission: [AUDIT, 'accounts:write'] })
  sync() {
    return null;
  }

  @Write('override')
  @Auth({ permission: 'accounts:read', ...overrides })
  override() {
    return null;
  }

  @Read('scoped')
  @RequireScope(PROJECTS_READ)
  scoped() {
    return null;
  }

  @Read('session')
  @Session()
  session() {
    return null;
  }
}
