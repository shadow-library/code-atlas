import { Api, Read, Write } from '@other/http';

@Api('lookalike')
export class OtherPackageController {
  @Read()
  list() {
    return [];
  }

  @Write('items')
  create() {
    return null;
  }
}
