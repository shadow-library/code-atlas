import { Api } from '@acme/http';
import { Read } from '@other/http';

@Api('mixed')
export class MixedController {
  @Read()
  list() {
    return [];
  }
}
