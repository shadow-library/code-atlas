import { Api, Public, Read } from '@acme/server';

@Api('status')
export class StatusController {
  @Read()
  @Public()
  status() {
    return 'ok';
  }
}
