import { Api, Auth, Authenticated, Public, Read, RequireScope } from '@acme/blend';

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

@Api('catalog')
@Public()
export class CatalogController {
  @Read('drafts')
  @Authenticated()
  drafts() {
    return [];
  }

  @Read('hooks')
  @Auth({ service: 'hooks' })
  hooks() {
    return null;
  }
}
