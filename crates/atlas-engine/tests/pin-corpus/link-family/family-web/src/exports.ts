import { ExportsRequest, kitClient } from './transport';

export const runExport = () => ExportsRequest.post('/run').execute();

export const findExport = (id: string) => ExportsRequest.get(`/${id}`).execute();

export const findOrder = (id: string) => kitClient.orders.get(`/${id}`).execute();

export const latestExportFile = (id: string, n: number) => ExportsRequest.get(`${draftPath(id, n)}/files/latest`).execute();

function draftPath(id: string, n: number): string {
  return `/${id}/drafts/${n}`;
}

export const anyLatestFile = (prefix: string) => fetch(`${prefix}/files/latest`);
