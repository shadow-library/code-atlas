import { Hono } from 'hono'
import type { LambdaEvent } from './handler'
import { defaultIsContentTypeBinary, handle } from './handler'

const TEXTUAL = [
  'text/markdown; charset=UTF-8',
  'text/event-stream',
  'text/calendar',
  'text/xml',
  'text/tab-separated-values',
  'TEXT/MARKDOWN',
  'application/javascript',
  'application/javascript; charset=utf-8',
  'application/x-www-form-urlencoded',
  'application/json',
  'image/svg+xml',
]

const BINARY = [
  'image/png',
  'application/octet-stream',
  'application/pdf',
  'application/zip',
  'application/vnd.apple.installer+xml',
  'application/vnd.mozilla.xul+xml',
  'font/woff2',
]

describe('aws-lambda textual content types', () => {
  it.each(TEXTUAL)('should treat %s as text', (contentType) => {
    expect(defaultIsContentTypeBinary(contentType)).toBe(false)
  })

  it.each(BINARY)('should treat %s as binary', (contentType) => {
    expect(defaultIsContentTypeBinary(contentType)).toBe(true)
  })

  it('should return a markdown response as plain text from handle()', async () => {
    const app = new Hono()
    app.get('/readme', (c) => c.body('# Title', 200, { 'Content-Type': 'text/markdown; charset=UTF-8' }))
    const handler = handle(app)
    const event = {
      version: '2.0',
      routeKey: '$default',
      rawPath: '/readme',
      rawQueryString: '',
      headers: { host: 'example.com' },
      body: null,
      isBase64Encoded: false,
      requestContext: { domainName: 'example.com', http: { method: 'GET' } },
    } as unknown as LambdaEvent

    const result = await handler(event)

    expect(result.isBase64Encoded).toBe(false)
    expect(result.body).toBe('# Title')
  })
})
