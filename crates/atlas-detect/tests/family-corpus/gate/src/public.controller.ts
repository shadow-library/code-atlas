import { Api, Auth, Lenient, Public, Read, RequireScope } from '@acme/gate';

@Api('public')
@Public()
export class PublicController {
  @Read()
  index() {
    return [];
  }

  @Read('admin')
  @Auth()
  admin() {
    return null;
  }

  @Read('scoped')
  @RequireScope('public:scoped')
  scoped() {
    return null;
  }
}

@Api('scopes')
export class ScopesController {
  @Read()
  @RequireScope('scopes:read')
  list() {
    return [];
  }

  @Read('lenient')
  @Lenient()
  lenient() {
    return null;
  }
}
