import { Api, Permit, Read } from '@acme/server';
import * as P from './permissions';
import { PROJECTS_WRITE as PW, Scope } from './permissions';

@Api('imports')
export class ImportsController {
  @Read('aliased')
  @Permit(PW)
  aliased() {
    return null;
  }

  @Read('namespaced')
  @Permit(P.PROJECTS_WRITE, P.Scope.Admin)
  namespaced() {
    return null;
  }

  @Read('member')
  @Permit(Scope.Admin)
  member() {
    return null;
  }
}
