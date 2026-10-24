import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const volatileNames = new Set(['CreatedAt','UpdatedAt','timestamp','requestId','uuid','token','captchaId','expiresAt','iat','exp','nbf'])
const pointer = key => String(key).replaceAll('~','~0').replaceAll('/','~1')
const valueType = value => value === null ? 'null' : Array.isArray(value) ? 'array' : typeof value

export function compareContract(expected,actual,normalizers=[]) {
  for (const path of normalizers) {
    const name = path.split('/').at(-1).replaceAll('~1','/').replaceAll('~0','~')
    if (!path.startsWith('/') || !volatileNames.has(name)) throw new Error(`Unsupported volatile normalizer: ${path}`)
  }
  const differences = []
  function walk(a,b,path) {
    const type = valueType(a)
    if (type !== valueType(b)) { differences.push({path,kind:'type',expected:a,actual:b});return }
    if (normalizers.includes(path)) {
      if (type === 'object' || type === 'array') throw new Error(`Volatile normalization must target a scalar: ${path}`)
      return
    }
    if (type === 'object' || type === 'array') {
      for (const key of [...new Set([...Object.keys(a),...Object.keys(b)])].sort()) {
        const child = `${path}/${pointer(key)}`
        if (!Object.hasOwn(a,key)) differences.push({path:child,kind:'additional',actual:b[key]})
        else if (!Object.hasOwn(b,key)) differences.push({path:child,kind:'missing',expected:a[key]})
        else walk(a[key],b[key],child)
      }
      if (type === 'array' && a.length !== b.length && !differences.some(d=>d.path.startsWith(`${path}/`))) differences.push({path,kind:'length',expected:a.length,actual:b.length})
    } else if (!Object.is(a,b)) differences.push({path,kind:'value',expected:a,actual:b})
  }
  walk(expected,actual,'')
  return differences
}

export function assertSafeFixture(value) {
  function inspect(item,path='') {
    if (typeof item === 'string' && item !== '<redacted>' && (/\bBearer\s+[^<\s]+/i.test(item) || /(?:mysql|postgres(?:ql)?):\/\/[^\s/:]+:[^\s@]+@/i.test(item))) throw new Error(`Fixture contains a credential at ${path}`)
    if (!item || typeof item !== 'object') return
    for (const [key,child] of Object.entries(item)) {
      const normalized = key.replace(/[-_]/g,'').toLowerCase()
      const sensitive = /^(?:password|passwd|token|accesstoken|refreshtoken|authorization|xtoken|signingkey|initsecret|secretkey|clientsecret|apikey)$/.test(normalized)
      if (sensitive && child !== '<redacted>' && child !== '' && child !== null) throw new Error(`Fixture contains a secret at ${path}/${pointer(key)}`)
      inspect(child,`${path}/${pointer(key)}`)
    }
  }
  inspect(value)
}

async function main() {
  const [expectedFile, actualFile, normalizersFile] = process.argv.slice(2)
  if (!expectedFile || !actualFile) throw new Error('Usage: node tools/compare-contracts.mjs expected.json actual.json [normalizers.json]')
  const expected = JSON.parse(await readFile(expectedFile,'utf8'))
  const actual = JSON.parse(await readFile(actualFile,'utf8'))
  assertSafeFixture(expected)
  assertSafeFixture(actual)
  const normalizers = normalizersFile ? JSON.parse(await readFile(normalizersFile,'utf8')) : []
  const differences = compareContract(expected,actual,normalizers)
  console.log(JSON.stringify(differences,null,2))
  if (differences.length) process.exitCode=1
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main().catch(error=>{console.error(error.message);process.exitCode=1})
