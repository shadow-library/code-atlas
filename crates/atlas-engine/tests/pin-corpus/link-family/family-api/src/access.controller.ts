import { Api, Authenticated, Guarded, OptionalAuth, Public, Read, RequireScope, Session } from '@acme/http';

import { SessionGuard } from './session.guard';

@Api('access')
@Authenticated()
export class AccessController {
  @Read('open')
  @Public()
  open() {
    return null;
  }

  @Read('maybe')
  @OptionalAuth()
  maybe() {
    return null;
  }

  @Read('hook')
  @Public()
  @Guarded(SessionGuard)
  hook() {
    return null;
  }
}

@Api('scoped')
export class ScopedController {
  @Read()
  @RequireScope('scoped:read')
  list() {
    return [];
  }

  @Read('session')
  @Session()
  session() {
    return null;
  }
}

@Api('docs')
@Public()
export class DocsController {
  @Read('drafts')
  @Authenticated()
  drafts() {
    return [];
  }
}
