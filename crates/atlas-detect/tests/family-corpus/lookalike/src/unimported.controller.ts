import { Injectable } from '@acme/http';

function Api(path: string) {
  return (target: object) => target;
}

function Read(path?: string) {
  return (target: object, key: string) => key;
}

@Injectable()
@Api('local')
export class LocalDecoratorsController {
  @Read()
  list() {
    return [];
  }
}
