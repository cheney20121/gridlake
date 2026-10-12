import { LEDGER_BASE } from '@/api/ledger-base'
import request from '@/utils/request'

/** MCP 接入：外部 AI 助手（Claude Desktop、Cursor、Cherry Studio、Dify…）通过令牌只读查询数据集与仪表盘 */
const url = (path: string) => `${LEDGER_BASE}/api/ledger/mcp${path}`

/** MCP 服务地址（Streamable HTTP） */
export const MCP_ENDPOINT = `${LEDGER_BASE}/api/mcp`

export interface McpToken {
  id: string
  name: string
  /** 令牌开头几位（辨认用） */
  prefix: string
  allowSql: boolean
  /** 可访问的数据集 ID；null 为全部 */
  datasets: string[] | null
  createdBy: string | null
  createdAt: string
  lastUsedAt: string | null
  calls: number
}

export interface McpTokenInput {
  name: string
  allowSql: boolean
  datasets: string[] | null
  operator?: string
}

export interface McpLog {
  tokenName: string
  method: string
  tool: string | null
  arguments: string | null
  ok: boolean
  message: string | null
  elapsedMs: number
  client: string | null
  createdAt: string
}

export function listMcpTokensApi() {
  return request<McpToken[]>({ url: url('/tokens'), method: 'get' })
}

/** 新建令牌：secret 为令牌明文，只返回这一次 */
export function createMcpTokenApi(data: McpTokenInput) {
  return request<{ token: McpToken, secret: string }>({ url: url('/tokens'), method: 'post', data })
}

export function updateMcpTokenApi(id: string, data: McpTokenInput) {
  return request<boolean>({ url: url(`/tokens/${id}`), method: 'put', data })
}

export function revokeMcpTokenApi(id: string) {
  return request<boolean>({ url: url(`/tokens/${id}`), method: 'delete' })
}

export function mcpLogsApi(limit = 200) {
  return request<McpLog[]>({ url: url('/logs'), method: 'get', params: { limit } })
}
