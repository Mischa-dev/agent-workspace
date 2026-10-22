import { createHash, randomBytes } from 'node:crypto'
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http'

/**
 * A stand-in for the few WorkOS endpoints single sign-on uses, so the browser
 * tests can run the whole sign-in round trip. It listens on `localhost` while
 * the app is on `127.0.0.1`, so the browser treats it as another site, the way
 * it treats the real WorkOS.
 */
export const workOsStandIn = {
  clientId: 'client_e2e',
  apiKey: `sk_e2e_${randomBytes(12).toString('hex')}`,
  user: {
    id: 'user_e2e_smoke',
    email: 'smoke@example.test',
    email_verified: true,
    first_name: 'Smoke',
    last_name: 'Tester',
  },
}

type Pending = { challenge: string; redirectUri: string }

export async function startWorkOsStandIn(port: number) {
  const codes = new Map<string, Pending>()
  // WorkOS session id -> still active.
  const sessions = new Map<string, boolean>()

  const server = createServer((request, response) => {
    handle(request, response).catch(() => send(response, 500, { message: 'stand-in failed' }))
  })

  async function handle(request: IncomingMessage, response: ServerResponse) {
    const url = new URL(request.url ?? '/', `http://localhost:${port}`)
    const authorized = request.headers.authorization === `Bearer ${workOsStandIn.apiKey}`

    if (request.method === 'GET' && url.pathname === '/user_management/authorize') {
      const redirectUri = url.searchParams.get('redirect_uri') ?? ''
      const state = url.searchParams.get('state') ?? ''
      const challenge = url.searchParams.get('code_challenge') ?? ''
      if (url.searchParams.get('client_id') !== workOsStandIn.clientId || !redirectUri || !state || !challenge) {
        return send(response, 400, { message: 'bad authorize request' })
      }
      // Straight back, as if the person had typed their email and code.
      const code = randomBytes(16).toString('hex')
      codes.set(code, { challenge, redirectUri })
      const back = new URL(redirectUri)
      back.searchParams.set('code', code)
      back.searchParams.set('state', state)
      response.writeHead(302, { location: back.toString() })
      return response.end()
    }

    if (request.method === 'POST' && url.pathname === '/user_management/authenticate') {
      const body = JSON.parse(await read(request)) as Record<string, string>
      const pending = codes.get(body.code)
      codes.delete(body.code)
      const verifierMatches =
        pending && createHash('sha256').update(body.code_verifier ?? '').digest('base64url') === pending.challenge
      if (
        !pending ||
        !verifierMatches ||
        body.client_id !== workOsStandIn.clientId ||
        body.client_secret !== workOsStandIn.apiKey ||
        body.grant_type !== 'authorization_code'
      ) {
        return send(response, 400, { error: 'invalid_grant' })
      }
      const sid = `session_e2e_${randomBytes(8).toString('hex')}`
      sessions.set(sid, true)
      const part = (value: object) => Buffer.from(JSON.stringify(value)).toString('base64url')
      return send(response, 200, {
        user: workOsStandIn.user,
        access_token: `${part({ alg: 'none' })}.${part({ sid, sub: workOsStandIn.user.id })}.signature`,
        refresh_token: 'unused',
      })
    }

    const listing = url.pathname.match(/^\/user_management\/users\/([^/]+)\/sessions$/)
    if (request.method === 'GET' && listing) {
      if (!authorized) return send(response, 401, { message: 'unauthorized' })
      const data =
        listing[1] === workOsStandIn.user.id
          ? [...sessions].map(([id, active]) => ({ id, status: active ? 'active' : 'revoked' }))
          : []
      return send(response, 200, { data, list_metadata: { before: null, after: null } })
    }

    if (request.method === 'POST' && url.pathname === '/user_management/sessions/revoke') {
      if (!authorized) return send(response, 401, { message: 'unauthorized' })
      const { session_id: sid } = JSON.parse(await read(request)) as { session_id: string }
      if (sessions.has(sid)) sessions.set(sid, false)
      return send(response, 200, {})
    }

    send(response, 404, { message: 'not part of the stand-in' })
  }

  await new Promise<void>((resolve) => server.listen(port, 'localhost', resolve))
  return {
    origin: `http://localhost:${port}`,
    close: () => new Promise<void>((resolve) => server.close(() => resolve())),
  }
}

function read(request: IncomingMessage) {
  return new Promise<string>((resolve, reject) => {
    let body = ''
    request.setEncoding('utf8')
    request.on('data', (chunk: string) => (body += chunk))
    request.on('end', () => resolve(body))
    request.on('error', reject)
  })
}

function send(response: ServerResponse, status: number, body: unknown) {
  response.writeHead(status, { 'content-type': 'application/json' })
  response.end(JSON.stringify(body))
}
