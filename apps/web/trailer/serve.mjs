// Serves the built trailer, plus the few images its messages attach, at the
// paths the app's own components ask for (/api/v1/files/<id>/thumbnail).
import { createReadStream, existsSync, statSync } from 'node:fs'
import { createServer } from 'node:http'
import { extname, join, normalize, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = fileURLToPath(new URL('.', import.meta.url))
const dist = join(here, 'dist')
const assets = join(here, 'assets')
const types = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.woff2': 'font/woff2', '.png': 'image/png', '.svg': 'image/svg+xml', '.jpg': 'image/jpeg' }

export function serve(port = 5199) {
  const server = createServer((request, response) => {
    const url = decodeURIComponent((request.url ?? '/').split('?')[0])
    const file = /^\/api\/v1\/files\/([0-9a-f-]+)\/(thumbnail|content)$/.exec(url)
    const path = file ? join(assets, `${file[1]}.png`) : normalize(join(dist, url === '/' ? 'index.html' : url))
    if ((!file && !path.startsWith(dist + sep)) || !existsSync(path) || !statSync(path).isFile()) {
      response.writeHead(404)
      response.end()
      return
    }
    response.writeHead(200, { 'content-type': types[extname(path)] ?? 'application/octet-stream' })
    createReadStream(path).pipe(response)
  })
  return new Promise((resolve) => server.listen(port, '127.0.0.1', () => resolve(server)))
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  await serve(Number(process.env.PORT ?? 5199))
  console.log('serving on http://127.0.0.1:5199')
}
