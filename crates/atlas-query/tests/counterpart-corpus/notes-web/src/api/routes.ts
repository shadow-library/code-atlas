export const ROUTE_NAMES = ['pages.search', 'pages.listing', 'pages.export_all'];

export function routeTitle(name: string): string {
  return name.replace('.', ' ');
}
