import { createServer } from 'node:http'
import { readFile, readdir, realpath } from 'node:fs/promises'
import { resolve, relative, sep, extname, isAbsolute } from 'node:path'
import { spawnSync } from 'node:child_process'
export async function generatedFrontend(input: string) {
  const allowed = await realpath(resolve('.superpowers')),
    project = await realpath(resolve(input)),
    local = relative(allowed, project)
  if (local === '..' || local.startsWith('..' + sep) || isAbsolute(local))
    throw new Error('Generated UI fixture must remain in the private workspace')
  const check = spawnSync(
    process.execPath,
    [resolve('tools/check-generated-manifest.mjs'), project],
    { windowsHide: true, encoding: 'utf8' },
  )
  if (check.status !== 0) throw new Error('Generated UI fixture manifest verification failed')
  const descriptor = JSON.parse(await readFile(resolve(project, 'schema.json'), 'utf8')) as {
    fields: Array<{ name: string }>
  }
  for (const name of ['sku', 'count', 'ratio', 'enabled', 'releasedAt', 'tags'])
    if (!descriptor.fields.some((field) => field.name === name))
      throw new Error('Generated UI fixture requires all six field kinds')
  const dist = resolve(project, 'web/dist'),
    files = new Map<string, Buffer>()
  let bytes = 0
  async function load(directory: string, prefix = '') {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      if (entry.isSymbolicLink()) throw new Error('Generated static fixture cannot contain links')
      if (entry.isDirectory()) await load(resolve(directory, entry.name), prefix + entry.name + '/')
      else {
        const value = await readFile(resolve(directory, entry.name))
        bytes += value.length
        if (bytes > 2 * 1024 * 1024 || files.size >= 64)
          throw new Error('Generated static fixture is too large')
        files.set(prefix + entry.name, value)
      }
    }
  }
  await load(dist)
  const server = createServer((request, response) => {
    if (request.method !== 'GET' && request.method !== 'HEAD') {
      response.writeHead(405)
      response.end()
      return
    }
    let path: string
    try {
      path =
        decodeURIComponent(new URL(request.url ?? '/', 'http://localhost').pathname).slice(1) ||
        'index.html'
    } catch {
      response.writeHead(400)
      response.end()
      return
    }
    const bytes = files.get(path)
    if (!bytes) {
      response.writeHead(404)
      response.end()
      return
    }
    const type =
      extname(path) === '.html'
        ? 'text/html; charset=utf-8'
        : extname(path) === '.js'
          ? 'text/javascript; charset=utf-8'
          : extname(path) === '.css'
            ? 'text/css; charset=utf-8'
            : 'application/octet-stream'
    response.writeHead(200, {
      'content-type': type,
      'content-length': bytes.length,
      'cache-control': 'no-store',
    })
    response.end(request.method === 'HEAD' ? undefined : bytes)
  })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  const address = server.address()
  if (!address || typeof address === 'string')
    throw new Error('Generated static fixture address missing')
  return {
    origin: `http://localhost:${address.port}`,
    schemaPath: resolve(project, 'schema.json'),
    close: async () => {
      server.closeAllConnections()
      await new Promise<void>((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      )
    },
  }
}
