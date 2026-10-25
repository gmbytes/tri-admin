import { spawnSync } from 'node:child_process'
import { randomBytes, randomUUID } from 'node:crypto'
import { mkdir, mkdtemp, realpath, writeFile, rm } from 'node:fs/promises'
import { resolve, join, sep } from 'node:path'

const image =
  'dxflrs/garage@sha256:677e5b758e4425a325b811c425c026bf35a4bf42f7ddbaa773de3f4fd779a5a5'
const workspace = await realpath(resolve('.')),
  root = join(workspace, '.superpowers', 's3')
await mkdir(root, { recursive: true })
const canonicalRoot = await realpath(root)
if (!canonicalRoot.startsWith(workspace + sep))
  throw Error('Refusing out-of-workspace S3 fixture root')
const directory = await mkdtemp(join(root, 'run-')),
  owner = randomUUID(),
  name = 'tri-admin-s3-' + owner
const bucket = 'tri-admin-' + randomBytes(8).toString('hex'),
  accessKeyId = 'GK' + randomBytes(16).toString('hex'),
  secretAccessKey = randomBytes(32).toString('hex')
let id = '',
  exitCode = 1
function docker(args, environment = process.env) {
  return spawnSync('docker', args, {
    env: environment,
    encoding: 'utf8',
    windowsHide: true,
    maxBuffer: 1024 * 1024,
  })
}
try {
  const config = join(directory, 'garage.toml')
  await writeFile(
    config,
    `metadata_dir = "/tmp/meta"\ndata_dir = "/tmp/data"\ndb_engine = "sqlite"\nreplication_factor = 1\nrpc_bind_addr = "[::]:3901"\nrpc_public_addr = "127.0.0.1:3901"\nrpc_secret = "${randomBytes(32).toString('hex')}"\n[s3_api]\ns3_region = "garage"\napi_bind_addr = "[::]:3900"\nroot_domain = ".s3.garage.localhost"\n`,
    { mode: 0o600 },
  )
  const started = docker(
    [
      'run',
      '--rm',
      '-d',
      '--name',
      name,
      '--label',
      'tri-admin.fixture=' + owner,
      '--memory',
      '512m',
      '--cpus',
      '1',
      '-p',
      '127.0.0.1::3900',
      '-v',
      config + ':/etc/garage.toml:ro',
      '-e',
      'GARAGE_DEFAULT_ACCESS_KEY',
      '-e',
      'GARAGE_DEFAULT_SECRET_KEY',
      '-e',
      'GARAGE_DEFAULT_BUCKET',
      image,
      '/garage',
      'server',
      '--single-node',
      '--default-bucket',
    ],
    {
      ...process.env,
      GARAGE_DEFAULT_ACCESS_KEY: accessKeyId,
      GARAGE_DEFAULT_SECRET_KEY: secretAccessKey,
      GARAGE_DEFAULT_BUCKET: bucket,
    },
  )
  if (started.status !== 0 || !/^[a-f0-9]{64}$/.test(started.stdout.trim()))
    throw Error('S3 fixture container could not start')
  id = started.stdout.trim()
  const inspect = docker(['inspect', id])
  if (inspect.status !== 0) throw Error('S3 fixture inspect failed')
  const detail = JSON.parse(inspect.stdout)[0]
  if (detail.Config.Labels['tri-admin.fixture'] !== owner || detail.Name !== '/' + name)
    throw Error('S3 fixture ownership mismatch')
  const binding = detail.NetworkSettings.Ports['3900/tcp']?.[0]
  if (binding?.HostIp !== '127.0.0.1' || !/^\d+$/.test(binding.HostPort))
    throw Error('S3 fixture must bind only loopback')
  const endpoint = 'http://127.0.0.1:' + binding.HostPort
  let ready = false
  const deadline = Date.now() + 30000
  while (Date.now() < deadline) {
    const checked = docker(['exec', id, '/garage', 'bucket', 'info', bucket])
    if (checked.status === 0) {
      ready = true
      break
    }
    await new Promise((resolve) => setTimeout(resolve, 200))
  }
  if (!ready) throw Error('S3 fixture did not become ready')
  const fixture = join(directory, 'fixture.json')
  await writeFile(
    fixture,
    JSON.stringify({ endpoint, region: 'garage', bucket, accessKeyId, secretAccessKey }),
    { mode: 0o600 },
  )
  console.log('Isolated S3 fixture ready; running real authentication acceptance test.')
  const result = spawnSync(
    'cargo',
    ['test', '-p', 'admin-api', '--test', 's3_connection', '--', '--ignored'],
    {
      env: { ...process.env, CARGO_INCREMENTAL: '0', TRI_S3_FIXTURE_FILE: fixture },
      stdio: 'inherit',
      windowsHide: true,
    },
  )
  exitCode = result.status ?? 1
} finally {
  if (id) {
    const inspected = docker(['inspect', id])
    if (inspected.status !== 0) throw Error('Cannot verify S3 fixture ownership for cleanup')
    const detail = JSON.parse(inspected.stdout)[0]
    if (detail.Config.Labels['tri-admin.fixture'] !== owner || detail.Name !== '/' + name)
      throw Error('Refusing to remove a non-owned container')
    const removed = docker(['rm', '-f', '-v', id])
    if (removed.status !== 0) throw Error('S3 fixture cleanup failed')
  }
  const canonical = await realpath(directory)
  if (
    !canonical.startsWith(canonicalRoot + sep) ||
    !canonical.split(sep).at(-1)?.startsWith('run-')
  )
    throw Error('Refusing out-of-scope fixture cleanup')
  await rm(directory, { recursive: true, force: true })
  console.log('S3 fixture container and temporary credentials removed.')
}
process.exitCode = exitCode
