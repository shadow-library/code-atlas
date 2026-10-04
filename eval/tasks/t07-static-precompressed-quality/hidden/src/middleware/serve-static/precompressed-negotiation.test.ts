import { serveStatic } from '.'
import { Hono } from '../../hono'

const ALL_VARIANTS = ['.br', '.zst', '.gz']

const appServing = (variants: string[]) => {
  const getContent = async (path: string) => {
    const variant = ALL_VARIANTS.find((suffix) => path.endsWith(suffix))
    if (variant && !variants.includes(variant)) {
      return null
    }
    return `content of ${path}`
  }
  return new Hono().use('*', serveStatic({ getContent, precompressed: true }))
}

const fetchWith = (app: Hono, acceptEncoding: string) =>
  app.request('/static/app.js', { headers: { 'Accept-Encoding': acceptEncoding } })

describe('serveStatic precompressed negotiation', () => {
  it('should choose the coding with the highest quality value', async () => {
    const res = await fetchWith(appServing(ALL_VARIANTS), 'br;q=0.2, gzip;q=0.8, zstd;q=0.5')
    expect(res.headers.get('Content-Encoding')).toBe('gzip')
    expect(res.headers.get('Vary')).toBe('Accept-Encoding')
    expect(await res.text()).toBe('content of static/app.js.gz')
  })

  it('should accept codings that carry an explicit q=1 parameter', async () => {
    const res = await fetchWith(appServing(ALL_VARIANTS), 'gzip;q=1.0, br;q=0.5')
    expect(res.headers.get('Content-Encoding')).toBe('gzip')
    expect(await res.text()).toBe('content of static/app.js.gz')
  })

  it('should never serve a coding the client refuses with q=0', async () => {
    const res = await fetchWith(appServing(['.br', '.gz']), 'br;q=0, gzip')
    expect(res.headers.get('Content-Encoding')).toBe('gzip')
    expect(await res.text()).toBe('content of static/app.js.gz')
  })

  it('should serve the uncompressed file when every available coding is refused', async () => {
    const res = await fetchWith(appServing(['.br']), 'br;q=0, gzip')
    expect(res.headers.get('Content-Encoding')).toBeNull()
    expect(res.headers.get('Vary')).toBeNull()
    expect(await res.text()).toBe('content of static/app.js')
  })

  it('should prefer br, then zstd, then gzip when quality values tie', async () => {
    const res = await fetchWith(appServing(ALL_VARIANTS), 'gzip;q=0.5, zstd;q=0.5, br;q=0.5')
    expect(res.headers.get('Content-Encoding')).toBe('br')
    expect(await res.text()).toBe('content of static/app.js.br')
  })

  it('should fall back to the next acceptable coding when the preferred file is missing', async () => {
    const res = await fetchWith(appServing(['.gz']), 'br, zstd;q=0.9, gzip;q=0.1')
    expect(res.headers.get('Content-Encoding')).toBe('gzip')
    expect(await res.text()).toBe('content of static/app.js.gz')
  })

  it('should treat the wildcard as accepting every coding not listed explicitly', async () => {
    const res = await fetchWith(appServing(ALL_VARIANTS), 'br;q=0, *;q=0.3')
    expect(res.headers.get('Content-Encoding')).toBe('zstd')
    expect(await res.text()).toBe('content of static/app.js.zst')
  })

  it('should match content codings case-insensitively', async () => {
    const res = await fetchWith(appServing(ALL_VARIANTS), 'GZIP')
    expect(res.headers.get('Content-Encoding')).toBe('gzip')
    expect(await res.text()).toBe('content of static/app.js.gz')
  })
})
