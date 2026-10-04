import { Hono } from '../../hono'
import { serialize } from '../../utils/cookie'
import {
  deleteCookie,
  generateCookie,
  generateSignedCookie,
  setCookie,
  setSignedCookie,
} from '.'

describe('cookie helper SameSite default', () => {
  it('should add SameSite=Lax when setCookie() is called without sameSite', async () => {
    const app = new Hono()
    app.get('/', (c) => {
      setCookie(c, 'session', 'abc')
      return c.text('ok')
    })
    const res = await app.request('/')
    expect(res.headers.get('Set-Cookie')).toBe('session=abc; Path=/; SameSite=Lax')
  })

  it('should keep an explicit sameSite option', async () => {
    const app = new Hono()
    app.get('/', (c) => {
      setCookie(c, 'strict', 'a', { sameSite: 'Strict' })
      setCookie(c, 'none', 'b', { sameSite: 'None', secure: true })
      return c.text('ok')
    })
    const res = await app.request('/')
    expect(res.headers.getSetCookie()).toEqual([
      'strict=a; Path=/; SameSite=Strict',
      'none=b; Path=/; Secure; SameSite=None',
    ])
  })

  it('should add SameSite=Lax to prefixed cookies', () => {
    expect(generateCookie('id', 'x', { prefix: 'secure' })).toBe(
      '__Secure-id=x; Path=/; Secure; SameSite=Lax'
    )
    expect(generateCookie('id', 'x', { prefix: 'host' })).toBe(
      '__Host-id=x; Path=/; Secure; SameSite=Lax'
    )
  })

  it('should add SameSite=Lax to signed cookies', async () => {
    const cookie = await generateSignedCookie('token', 'v', 'secret', { httpOnly: true })
    expect(cookie).toMatch(/^token=v\.[^;]+; Path=\/; HttpOnly; SameSite=Lax$/)

    const securePrefixed = await generateSignedCookie('token', 'v', 'secret', { prefix: 'secure' })
    expect(securePrefixed).toMatch(/^__Secure-token=v\.[^;]+; Path=\/; Secure; SameSite=Lax$/)

    const app = new Hono()
    app.get('/', async (c) => {
      await setSignedCookie(c, 'token', 'v', 'secret', { prefix: 'host' })
      return c.text('ok')
    })
    const res = await app.request('/')
    expect(res.headers.get('Set-Cookie')).toMatch(/^__Host-token=v\.[^;]+; Path=\/; Secure; SameSite=Lax$/)
  })

  it('should add SameSite=Lax when deleting a cookie', async () => {
    const app = new Hono()
    app.get('/', (c) => {
      deleteCookie(c, 'session')
      return c.text('ok')
    })
    const res = await app.request('/')
    expect(res.headers.get('Set-Cookie')).toBe('session=; Max-Age=0; Path=/; SameSite=Lax')
  })

  it('should leave the low-level serializer without a default', () => {
    expect(serialize('raw', 'value')).toBe('raw=value')
  })
})
