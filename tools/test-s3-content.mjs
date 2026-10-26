import { spawnSync } from 'node:child_process'
import { randomBytes, randomUUID } from 'node:crypto'
import { mkdir, mkdtemp, realpath, writeFile, rm } from 'node:fs/promises'
import { resolve, join, sep } from 'node:path'
const image =
  'chrislusf/seaweedfs@sha256:c2658820e763294a9e9de7b36c81e59b879e3459148351aee4e31a126ca64860'
const args = process.argv.slice(2)
if (args.some((value) => value !== '--exports-only') || args.length > 1)
  throw Error('Supported option: --exports-only')
const exportsOnly = args.includes('--exports-only')
const workspace = await realpath(resolve('.')),
  root = join(workspace, '.superpowers', 's3')
await mkdir(root, { recursive: true })
const canonicalRoot = await realpath(root)
if (!canonicalRoot.startsWith(workspace + sep))
  throw Error('Refusing out-of-workspace fixture root')
const directory = await mkdtemp(join(root, 'run-')),
  owner = randomUUID(),
  name = 'tri-admin-s3-' + owner
const accessKeyId = 'GK' + randomBytes(16).toString('hex'),
  secretAccessKey = randomBytes(32).toString('hex'),
  bucket = 'tri-admin-' + randomBytes(8).toString('hex')
let id = '',
  exitCode = 1
function docker(args, env = process.env, input) {
  return spawnSync('docker', args, {
    env,
    input,
    encoding: 'utf8',
    windowsHide: true,
    maxBuffer: 1024 * 1024,
    timeout: 20000,
  })
}
try {
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
      '768m',
      '--cpus',
      '1',
      '-p',
      '127.0.0.1::8333',
      '-e',
      'AWS_ACCESS_KEY_ID',
      '-e',
      'AWS_SECRET_ACCESS_KEY',
      '-e',
      'S3_BUCKET',
      image,
      'mini',
      '-dir=/data',
      '-volume.max=1',
      '-admin.ui=false',
    ],
    {
      ...process.env,
      AWS_ACCESS_KEY_ID: accessKeyId,
      AWS_SECRET_ACCESS_KEY: secretAccessKey,
      S3_BUCKET: bucket,
    },
  )
  if (started.status !== 0 || !/^[a-f0-9]{64}$/.test(started.stdout.trim()))
    throw Error('S3 content fixture could not start')
  id = started.stdout.trim()
  const inspected = docker(['inspect', id])
  if (inspected.status !== 0) throw Error('S3 content fixture inspect failed')
  const detail = JSON.parse(inspected.stdout)[0]
  if (detail.Config.Labels['tri-admin.fixture'] !== owner || detail.Name !== '/' + name)
    throw Error('S3 content fixture ownership mismatch')
  const binding = detail.NetworkSettings.Ports['8333/tcp']?.[0]
  if (binding?.HostIp !== '127.0.0.1' || !/^\d+$/.test(binding.HostPort))
    throw Error('S3 fixture must bind only loopback')
  let ready = false
  const deadline = Date.now() + 45000
  while (Date.now() < deadline) {
    const checked = docker(['exec', '-i', id, 'weed', 'shell'], process.env, 's3.bucket.list\n')
    if (checked.status === 0 && checked.stdout.includes(bucket)) {
      ready = true
      break
    }
    await new Promise((resolve) => setTimeout(resolve, 250))
  }
  if (!ready) throw Error('S3 content fixture did not become ready')
  const fixture = join(directory, 'fixture.json')
  await writeFile(
    fixture,
    JSON.stringify({
      endpoint: 'http://127.0.0.1:' + binding.HostPort,
      region: 'us-east-1',
      bucket,
      accessKeyId,
      secretAccessKey,
    }),
    { mode: 0o600 },
  )
  console.log('Isolated SeaweedFS 4.50 fixture ready; testing real immutable object storage.')
  const result = exportsOnly
    ? { status: 0 }
    : spawnSync(
        'cargo',
        [
          'test',
          '-p',
          'admin-api',
          '--test',
          'file_storage',
          'real_s3_content_writes',
          '--',
          '--ignored',
        ],
        {
          env: { ...process.env, CARGO_INCREMENTAL: '0', TRI_S3_FIXTURE_FILE: fixture },
          stdio: 'inherit',
          windowsHide: true,
        },
      )
  exitCode = result.status ?? 1
  if (exitCode === 0 && !exportsOnly) {
    const publication = spawnSync(
      'cargo',
      [
        'test',
        '-p',
        'admin-api',
        '--test',
        'file_publication',
        'real_s3_published_files',
        '--',
        '--ignored',
      ],
      {
        env: { ...process.env, CARGO_INCREMENTAL: '0', TRI_S3_FIXTURE_FILE: fixture },
        stdio: 'inherit',
        windowsHide: true,
      },
    )
    exitCode = publication.status ?? 1
  }
  if (exitCode === 0) {
    const exports = spawnSync(
      'cargo',
      ['test', '-p', 'admin-api', '--test', 'export_s3', '--', '--ignored', '--test-threads=1'],
      {
        env: { ...process.env, CARGO_INCREMENTAL: '0', TRI_S3_FIXTURE_FILE: fixture },
        stdio: 'inherit',
        windowsHide: true,
      },
    )
    exitCode = exports.status ?? 1
  }
} finally {
  if (id) {
    const inspected = docker(['inspect', id])
    if (inspected.status !== 0)
      throw Error('Cannot verify S3 content fixture ownership for cleanup')
    const detail = JSON.parse(inspected.stdout)[0]
    if (detail.Config.Labels['tri-admin.fixture'] !== owner || detail.Name !== '/' + name)
      throw Error('Refusing to remove a non-owned container')
    if (docker(['rm', '-f', '-v', id]).status !== 0)
      throw Error('S3 content fixture cleanup failed')
  }
  const canonical = await realpath(directory)
  if (
    !canonical.startsWith(canonicalRoot + sep) ||
    !canonical.split(sep).at(-1)?.startsWith('run-')
  )
    throw Error('Refusing out-of-scope fixture cleanup')
  await rm(canonical, { recursive: true, force: true })
  console.log('S3 content fixture and temporary credentials removed.')
}
process.exitCode = exitCode
