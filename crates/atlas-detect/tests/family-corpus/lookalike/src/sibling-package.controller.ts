import { Api, Read } from '@acme/http-client';

@Api('client')
export class SiblingPackageController {
  @Read()
  list() {
    return [];
  }
}
