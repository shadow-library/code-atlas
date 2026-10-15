import { Api, Auth, Authenticated, Guarded, OptionalAuth, Public, Read, Write } from '@acme/gate';

import { WebhookGuard } from './webhook.guard';

@Api('hooks')
@Auth()
export class HooksController {
  @Write('stripe')
  @Public()
  @Guarded(WebhookGuard)
  stripe() {
    return null;
  }

  @Read('maybe')
  @OptionalAuth()
  @Guarded(WebhookGuard)
  maybe() {
    return null;
  }
}

@Api('signed')
@Public()
@Guarded(WebhookGuard)
export class SignedController {
  @Write()
  receive() {
    return null;
  }

  @Read('status')
  @Authenticated()
  status() {
    return null;
  }
}

@Api('overrides')
@Auth({ public: true })
export class OverridesController {
  @Read('narrowed')
  @Auth({ permission: 'overrides:read' })
  narrowed() {
    return null;
  }

  @Read('inherited')
  inherited() {
    return null;
  }
}

@Api('reverse')
@Auth({ permission: 'reverse:admin' })
export class ReverseController {
  @Read('opened')
  @Auth({ public: true })
  opened() {
    return null;
  }

  @Read('kept')
  kept() {
    return null;
  }
}
