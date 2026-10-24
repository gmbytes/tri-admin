import { readdir, readFile } from 'node:fs/promises'
import { join } from 'node:path'

export async function sourceFiles(root, suffix) {
  const entries = await readdir(root, { withFileTypes: true }).catch(error => {
    if (error.code === 'ENOENT') return []
    throw error
  })
  const paths = await Promise.all(entries.map(entry => entry.isDirectory()
    ? sourceFiles(join(root, entry.name), suffix)
    : entry.name.endsWith(suffix) && !entry.name.endsWith('_test.go') ? [join(root, entry.name)] : []))
  return paths.flat().sort()
}

export function withoutComments(text) {
  return text.replace(/("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`)|\/\*[\s\S]*?\*\/|\/\/[^\r\n]*/g,
    (match, quoted) => quoted ?? match.replace(/[^\r\n]/g, ' '))
}

export async function optionalText(path) {
  return readFile(path, 'utf8').catch(error => {
    if (error.code === 'ENOENT') return ''
    throw error
  })
}
