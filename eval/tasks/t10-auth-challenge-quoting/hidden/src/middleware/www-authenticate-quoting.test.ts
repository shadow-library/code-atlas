import { Hono } from '../hono'
import { basicAuth } from './basic-auth'
import { bearerAuth } from './bearer-auth'
import { jwk } from './jwk'
import { jwt } from './jwt'

const REALM = String.raw`C:\vault "main" \"zone\"`
const QUOTED_REALM = String.raw`C:\\vault \"main\" \\\"zone\\\"`

const challenge = async (app: Hono, headers: Record<string, string> = {}) => {
  const res = await app.request('/protected', { headers })
  return { status: res.status, header: res.headers.get('WWW-Authenticate') }
}

describe('WWW-Authenticate quoted-string parameters', () => {
  it('should escape backslashes and quotes in the basic auth realm', async () => {
    const app = new Hono()
    app.use('/protected', basicAuth({ username: 'u', password: 'p', realm: REALM }))
    app.get('/protected', (c) => c.text('ok'))

    expect(await challenge(app)).toEqual({ status: 401, header: `Basic realm="${QUOTED_REALM}"` })
  })

  it('should escape backslashes and quotes in the bearer auth realm', async () => {
    const app = new Hono()
    app.use('/protected', bearerAuth({ token: 'secret-token', realm: REALM }))
    app.get('/protected', (c) => c.text('ok'))

    expect(await challenge(app)).toEqual({ status: 401, header: `Bearer realm="${QUOTED_REALM}"` })
  })

  it('should escape values of bearer auth challenges given as an object', async () => {
    const app = new Hono()
    app.use(
      '/protected',
      bearerAuth({
        token: 'secret-token',
        invalidToken: {
          wwwAuthenticateHeader: { error: 'invalid_token', error_description: String.raw`bad "token" \ expired` },
        },
      })
    )
    app.get('/protected', (c) => c.text('ok'))

    expect(await challenge(app, { Authorization: 'Bearer wrong-token' })).toEqual({
      status: 401,
      header: String.raw`Bearer error="invalid_token",error_description="bad \"token\" \\ expired"`,
    })
  })

  it('should keep a bearer auth challenge given as a string untouched', async () => {
    const raw = String.raw`Bearer realm="as\is"`
    const app = new Hono()
    app.use(
      '/protected',
      bearerAuth({ token: 'secret-token', noAuthenticationHeader: { wwwAuthenticateHeader: raw } })
    )
    app.get('/protected', (c) => c.text('ok'))

    expect(await challenge(app)).toEqual({ status: 401, header: raw })
  })

  it('should escape backslashes and quotes in the jwt realm', async () => {
    const app = new Hono()
    app.use('/protected', jwt({ secret: 'a-secret', alg: 'HS256', realm: REALM }))
    app.get('/protected', (c) => c.text('ok'))

    expect(await challenge(app)).toEqual({
      status: 401,
      header: `Bearer realm="${QUOTED_REALM}",error="invalid_request",error_description="no authorization included in request"`,
    })
  })

  it('should escape backslashes and quotes in the jwk realm', async () => {
    const app = new Hono()
    app.use('/protected', jwk({ keys: [], alg: ['RS256'], realm: REALM }))
    app.get('/protected', (c) => c.text('ok'))

    expect(await challenge(app)).toEqual({
      status: 401,
      header: `Bearer realm="${QUOTED_REALM}",error="invalid_request",error_description="no authorization included in request"`,
    })
  })
})
