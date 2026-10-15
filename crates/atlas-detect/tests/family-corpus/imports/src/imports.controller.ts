import { Api, Route } from '@acme/http';
import { EXPORT_OPTIONS } from './route-options';

const IMPORT_OPTIONS = { method: 'POST', path: 'run' } as const;

const STATUS_PATH = 'status/:jobId';

@Api('imports')
export class ImportsController {
  @Route(IMPORT_OPTIONS)
  run() {
    return true;
  }

  @Route({ method: 'get', path: STATUS_PATH })
  status() {
    return 'queued';
  }

  @Route({ method: Verb.PUT, path: ['retry/:jobId', 'resume/:jobId'] })
  retry() {
    return true;
  }

  @Route({ method: 'DELETE' })
  cancelAll() {
    return 0;
  }

  @Route(EXPORT_OPTIONS)
  exported() {
    return null;
  }

  @Route({ path: 'unknown' })
  noMethod() {
    return null;
  }

  @Route({ ...DEFAULT_OPTIONS, method: 'PATCH', path: 'draft', path: 'touch' })
  touch() {
    return true;
  }

  @Route({ method: 'POST', path: 'archive', ...overrides })
  archive() {
    return true;
  }
}
