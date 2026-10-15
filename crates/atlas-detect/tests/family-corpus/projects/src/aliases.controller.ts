import { Api, Read, Authenticated as Signed } from '@acme/server';
import * as server from '@acme/server';
import { Auth } from './local-auth';

@Api('aliases')
@Signed()
export class AliasesController {
  @Read()
  @server.Auth({ roles: ['viewer'] })
  @Auth()
  list() {
    return [];
  }
}
