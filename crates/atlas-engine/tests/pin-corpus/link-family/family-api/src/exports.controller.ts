import { Api, Read, Route } from '@acme/http';

const RUN_EXPORT = { method: 'POST', path: 'run' } as const;

@Api('exports')
export class ExportsController {
  @Route(RUN_EXPORT)
  run() {
    return true;
  }

  @Route({ method: 'GET', path: ':exportId' })
  find() {
    return null;
  }

  @Read(':exportId/drafts/:n/files/latest')
  latestFile() {
    return null;
  }
}
