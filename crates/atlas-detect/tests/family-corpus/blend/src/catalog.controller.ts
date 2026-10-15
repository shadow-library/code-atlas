import { Api, Auth, Authenticated, Public, Read, RequireScope } from '@acme/blend';

@Api('catalog')
@Public()
export class CatalogController {
  @Read()
  list() {
    return [];
  }

  @Read('drafts')
  @Authenticated()
  drafts() {
    return [];
  }

  @Read('scoped')
  @RequireScope('catalog:write')
  scoped() {
    return null;
  }

  @Read('hooks')
  @Auth({ service: 'hooks' })
  hooks() {
    return null;
  }
}

@Api('vault')
@Authenticated()
@RequireScope('vault:read')
export class VaultController {
  @Read()
  list() {
    return [];
  }

  @Read('service')
  @Auth({ service: 'vault' })
  service() {
    return null;
  }
}
