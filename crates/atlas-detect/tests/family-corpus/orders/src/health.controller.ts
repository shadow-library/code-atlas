import * as http from '@acme/http';
import { Read as Fetch } from '@acme/http';

@http.Api('health')
export class HealthController {
  @http.Read()
  status() {
    return 'ok';
  }

  @Fetch('ready')
  ready() {
    return true;
  }
}
