export function normalizeEndpoint(raw: string): string {
  let url: URL
  try {
    url = new URL(raw.trim())
  } catch {
    throw new Error('请输入完整的服务器地址')
  }
  const loopback = ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname)
  if (url.protocol !== 'https:' && !(url.protocol === 'http:' && loopback))
    throw new Error('服务器需要 HTTPS，本机开发地址可以使用 HTTP')
  if (url.username || url.password) throw new Error('服务器地址不能包含账号或密码')
  if (url.search || url.hash || url.port === '0')
    throw new Error('服务器地址不能包含查询、片段或无效端口')
  const path = url.pathname.replace(/\/+$/, '')
  if (path !== '' && path !== '/api/v1') throw new Error('请填写服务器根地址或 /api/v1 地址')
  return `${url.origin}/api/v1`
}
