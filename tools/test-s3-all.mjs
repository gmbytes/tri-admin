import { spawnSync } from 'node:child_process'
for (const script of ['tools/test-s3.mjs', 'tools/test-s3-content.mjs']) {
  const result = spawnSync(process.execPath, [script], { stdio: 'inherit', windowsHide: true })
  if (result.status !== 0) {
    process.exitCode = result.status ?? 1
    break
  }
}
