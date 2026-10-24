import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { optionalText, sourceFiles, withoutComments } from './source-utils.mjs'

const METHODS = new Set(['GET','POST','PUT','PATCH','DELETE','HEAD','OPTIONS','CONNECT','TRACE'])

export function normalizeRoutePath(path) {
  if (typeof path !== 'string' || !path.startsWith('/') || path.startsWith('//')) throw new Error('Expected an absolute route path')
  return path.split('?')[0].replace(/\{([^/{}]+)\}/g, ':$1').replace(/\/{2,}/g, '/').replace(/\/$/, '') || '/'
}

function methodName(value) {
  const method = String(value).toUpperCase()
  if (!METHODS.has(method)) throw new Error(`Unsupported HTTP method: ${method}`)
  return method
}

function middlewareAuth(expression, current = 'public') {
  if (/OpenUserAuth/.test(expression)) return 'open-user-token'
  if (/InitGuard/.test(expression)) return 'init-guard'
  if (/JWTAuth|CasbinHandler/.test(expression)) return 'jwt+casbin'
  return current
}

async function extractSources(sourceRoot) {
  const initialization = withoutComments(await optionalText(join(sourceRoot, 'server/initialize/router.go')))
  const callAuth = new Map()
  for (const match of initialization.matchAll(/\.(Init\w+)\(([^)]*)\)/g)) {
    callAuth.set(match[1], match[2].split(',').map(arg => /Private/.test(arg) ? 'jwt+casbin' : 'public'))
  }
  const files = await sourceFiles(join(sourceRoot, 'server/router'), '.go')
  files.push(join(sourceRoot, 'server/initialize/router.go'))
  const found = []
  for (const file of files) {
    const text = withoutComments(await optionalText(file))
    let groups = new Map([['Router', {path:'/',auth:'public'}]])
    const lines = text.split(/\r?\n/)
    for (let index = 0; index < lines.length; index++) {
      const line = lines[index]
      const fn = line.match(/^func\s+(?:\([^)]*\)\s+)?(\w+)\(([^)]*)\)/)
      if (fn) {
        groups = new Map([['Router', {path:'/',auth:'public'}]])
        const parameters = [...fn[2].matchAll(/(\w+)\s+\*gin\.RouterGroup/g)]
        parameters.forEach((parameter, i) => groups.set(parameter[1], {path:'/',auth:callAuth.get(fn[1])?.[i] ?? 'unknown'}))
      }
      const group = line.match(/(\w+)\s*:?=\s*(\w+)\.Group\(\s*"([^"]*)"\s*\)(.*)/)
      if (group) {
        const parent = groups.get(group[2])
        if (parent) groups.set(group[1], {path:normalizeRoutePath(`${parent.path.replace(/\/$/,'')}/${group[3].replace(/^\//,'')}`),auth:middlewareAuth(group[4],parent.auth)})
      }
      const route = line.match(/(\w+)\.(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\(\s*"([^"]*)"\s*,(.*)/)
      if (!route) continue
      const parent = groups.get(route[1])
      if (!parent) continue
      found.push({method:route[2],path:normalizeRoutePath(`${parent.path.replace(/\/$/,'')}/${route[3].replace(/^\//,'')}`),auth:middlewareAuth(route[4],parent.auth),
        evidence:{kind:'source',file:relative(sourceRoot,file).replaceAll('\\','/'),line:index+1}})
    }
  }
  return found
}

async function extractFrontend(sourceRoot) {
  const found = []
  for (const file of await sourceFiles(join(sourceRoot, 'web/src/api'), '.js')) {
    const text = withoutComments(await readFile(file, 'utf8'))
    for (const call of text.matchAll(/\bservice\(\s*\{([\s\S]*?)\}\s*\)/g)) {
      const url = call[1].match(/\burl\s*:\s*['"](\/[^'"]*)['"]/)?.[1]
      const method = call[1].match(/\bmethod\s*:\s*['"]([^'"]+)['"]/)?.[1]
      if (url && method) found.push({method:methodName(method),path:normalizeRoutePath(url),
        evidence:{kind:'frontend',file:relative(sourceRoot,file).replaceAll('\\','/'),line:text.slice(0,call.index).split('\n').length}})
    }
  }
  return found
}

export async function inventory({sourceRoot, runtimeRoutes = [], swaggerPath = join(sourceRoot, 'server/docs/swagger.json')}) {
  const records = new Map()
  const hasRuntime = runtimeRoutes.length > 0
  function add({method,path,auth,evidence}) {
    method = methodName(method)
    path = normalizeRoutePath(path)
    const key = `${method} ${path}`
    let record = records.get(key)
    if (!record) {
      record = {method,path,auth:auth ?? 'unknown',module:path.split('/')[1] || 'root',owner:'go',status:'pending',registered:hasRuntime ? false : null,documented:false,evidence:[]}
      records.set(key, record)
    }
    if (auth && auth !== 'unknown') record.auth = auth
    if (evidence.kind === 'runtime') {
      if (record.registered) throw new Error(`Duplicate runtime route: ${key}`)
      record.registered = true
    }
    if (evidence.kind === 'swagger') record.documented = true
    record.evidence.push(evidence)
  }
  for (const route of runtimeRoutes) add({...route,evidence:{kind:'runtime',...(route.handler ? {handler:route.handler} : {})}})
  for (const route of await extractSources(sourceRoot)) add(route)
  const swaggerText = await optionalText(swaggerPath)
  const swagger = swaggerText ? JSON.parse(swaggerText) : {paths:{}}
  for (const [path, operations] of Object.entries(swagger.paths ?? {})) {
    for (const [method, operation] of Object.entries(operations)) {
      if (!METHODS.has(method.toUpperCase())) continue
      add({method,path,evidence:{kind:'swagger',file:relative(sourceRoot,swaggerPath).replaceAll('\\','/'),...(operation.operationId ? {operationId:operation.operationId} : {})}})
    }
  }
  for (const route of await extractFrontend(sourceRoot)) add(route)
  const result = [...records.values()].sort((a,b) => a.path.localeCompare(b.path,'en') || a.method.localeCompare(b.method,'en'))
  for (const record of result) if (record.registered === false) record.status = 'unregistered'
  return result
}

async function main() {
  const args = process.argv.slice(2)
  const option = (name, fallback) => args.includes(name) ? args[args.indexOf(name)+1] : fallback
  const sourceRoot = resolve(option('--source','../vue-admin'))
  const runtimeText = await optionalText(resolve(option('--runtime','contracts/runtime-routes.json')))
  const routes = await inventory({sourceRoot,runtimeRoutes:runtimeText ? JSON.parse(runtimeText) : []})
  const output = resolve(option('--out','contracts/routes.json'))
  const serialized = `${JSON.stringify(routes,null,2)}\n`
  if (args.includes('--check')) {
    if (await optionalText(output) !== serialized) throw new Error('Route inventory has changed; regenerate and review contracts/routes.json')
  } else {
    await mkdir(dirname(output),{recursive:true})
    await writeFile(output,serialized)
  }
  console.log(`${routes.length} inventory entries; ${routes.filter(route=>route.registered).length} runtime routes; ${routes.filter(route=>route.documented).length} documented operations`)
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main().catch(error=>{console.error(error.message);process.exitCode=1})
