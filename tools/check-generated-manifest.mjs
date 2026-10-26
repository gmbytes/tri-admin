import { readFile, lstat } from 'node:fs/promises'
import { resolve, relative, sep } from 'node:path'
import { createHash } from 'node:crypto'
const root = resolve(process.argv[2] ?? ''),
  manifest = JSON.parse(await readFile(resolve(root, 'manifest.json'), 'utf8'))
if (
  manifest.templateVersion !== 1 ||
  !/^[a-f0-9]{64}$/.test(manifest.schemaChecksum) ||
  !Array.isArray(manifest.files) ||
  manifest.files.length > 64
)
  throw Error('Invalid generated manifest')
const names = new Set()
let size = 0
for (const file of manifest.files) {
  if (
    typeof file.path !== 'string' ||
    file.path.includes('\\') ||
    file.path.includes(':') ||
    file.path.split('/').some((part) => part === '..' || !part) ||
    names.has(file.path)
  )
    throw Error('Unsafe generated path')
  names.add(file.path)
  const path = resolve(root, file.path),
    local = relative(root, path)
  if (
    local.startsWith('..' + sep) ||
    local === '..' ||
    resolve(path) === root ||
    (await lstat(path)).isSymbolicLink()
  )
    throw Error('Unsafe generated file')
  const bytes = await readFile(path)
  if (
    bytes.length !== file.size ||
    createHash('sha256').update(bytes).digest('hex') !== file.sha256
  )
    throw Error('Generated file integrity mismatch')
  size += bytes.length
}
const cargo = await readFile(resolve(root, 'Cargo.lock'), 'utf8'),
  npm = JSON.parse(await readFile(resolve(root, 'web/package-lock.json'), 'utf8'))
if (
  !cargo.includes('name = "generated-admin"') ||
  !cargo.includes('[[package]]') ||
  npm.lockfileVersion !== 3 ||
  npm.packages?.['']?.name !== 'generated-admin-web'
)
  throw Error('Generated dependencies are not locked')
console.log(
  JSON.stringify({
    files: manifest.files.length,
    sourceBytes: size,
    schemaChecksum: manifest.schemaChecksum,
    locked: true,
  }),
)
