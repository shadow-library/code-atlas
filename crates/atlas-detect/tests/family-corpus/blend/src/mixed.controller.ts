import { Api, Auth, Authenticated, Public, Read } from '@acme/blend';

@Api('mixed')
@Auth({ public: true })
export class MixedController {
  @Read('a')
  @Authenticated()
  a() {
    return null;
  }

  @Read('b')
  @Auth({ permission: 'mixed:read' })
  @Authenticated()
  b() {
    return null;
  }

  @Read('open')
  open() {
    return null;
  }
}

@Api('order')
export class OrderController {
  @Read('guard-first')
  @Authenticated()
  @Public()
  guardFirst() {
    return null;
  }

  @Read('marker-first')
  @Public()
  @Authenticated()
  markerFirst() {
    return null;
  }

  @Read('unequal')
  @Public()
  @Authenticated()
  @Auth({ service: 'order' })
  unequal() {
    return null;
  }
}

@Api('lock')
@Authenticated()
@Public()
export class LockController {
  @Read()
  list() {
    return [];
  }
}
