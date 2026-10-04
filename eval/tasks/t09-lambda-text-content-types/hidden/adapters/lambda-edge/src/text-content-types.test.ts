import { Hono } from 'hono'
import type { CloudFrontEdgeEvent } from './handler'
import { handle, isContentTypeBinary } from './handler'

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

describe('lambda-edge textual content types', () => {
  it.each(TEXTUAL)('should treat %s as text', (contentType) => {
    expect(isContentTypeBinary(contentType)).toBe(false)
  })

  it.each(BINARY)('should treat %s as binary', (contentType) => {
    expect(isContentTypeBinary(contentType)).toBe(true)
  })

  it('should return a server-sent event payload without base64 body encoding', async () => {
    const app = new Hono()
    app.get('/events', (c) => c.body('data: hello\n\n', 200, { 'Content-Type': 'text/event-stream' }))
    const handler = handle(app)
    const event: CloudFrontEdgeEvent = {
      Records: [
        {
          cf: {
            config: {
              distributionDomainName: 'd111111abcdef8.cloudfront.net',
              distributionId: 'EDFDVBD6EXAMPLE',
              eventType: 'viewer-request',
              requestId: 'request-id',
            },
            request: {
              clientIp: '1.2.3.4',
              headers: { host: [{ key: 'Host', value: 'hono.dev' }] },
              method: 'GET',
              querystring: '',
              uri: '/events',
            },
          },
        },
      ],
    }

    const res = await handler(event)

    expect(res).not.toHaveProperty('bodyEncoding')
    expect(res).toMatchObject({ status: '200', body: 'data: hello\n\n' })
  })
})
