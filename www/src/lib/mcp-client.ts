import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js'
import { SSEClientTransport } from '@modelcontextprotocol/sdk/client/sse.js'

// Patch fetch to include credentials for MCP requests
const originalFetch = globalThis.fetch;
globalThis.fetch = function (input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
  const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
  if (url.includes('3001') || url.includes('localhost:3001')) {
    init = { ...init, credentials: 'include' };
  }
  return originalFetch.call(this, input, init);
};

// Minimal singleton-style MCP client for the browser
type TransportMode = 'streamable-http' | 'sse'

class McpClient {
  private client: Client | null = null
  private baseUrl: string
  private mode: TransportMode
  private connectionError: Error | null = null

  constructor(baseUrl: string, mode: TransportMode) {
    this.baseUrl = baseUrl
    this.mode = mode
  }

  async connect(): Promise<Client> {
    if (this.client) return this.client
    if (this.connectionError) throw this.connectionError

    try {
      const url = new URL(this.baseUrl)
      const client = new Client({
        name: 'ark-ui',
        version: '0.0.1'
      })

      let transport
      if (this.mode === 'sse') {
        transport = new SSEClientTransport(url)
      } else {
        transport = new StreamableHTTPClientTransport(url)
      }
      await client.connect(transport)

      this.client = client
      return this.client
    } catch (error) {
      console.error('MCP connection failed:', error)
      this.connectionError = error as Error
      throw error
    }
  }

  async listTools() {

    try {
      const c = await this.connect()
      const result = await c.listTools()
      return result
    } catch (error) {
      console.error('Failed to list tools:', error)
      throw error
    }
  }

  async callTool(name: string, args?: Record<string, any>) {


    try {
      const c = await this.connect()
      return c.callTool({ name, arguments: args ?? {} })
    } catch (error) {
      console.error(`Failed to call tool ${name}:`, error)
      throw error
    }
  }

  getConnectionError(): Error | null {
    return this.connectionError
  }

  isConnected(): boolean {
    return this.client !== null && this.connectionError === null
  }
}

// Factory with caching by base URL (supports future multi-server)
const cache = new Map<string, McpClient>()
export function getMcp(baseUrl: string, mode: TransportMode) {
  const key = `${baseUrl}|${mode}`
  if (!cache.has(key)) cache.set(key, new McpClient(baseUrl, mode))
  return cache.get(key)!
}

export type McpToolResult = Awaited<ReturnType<McpClient['callTool']>>
export type McpToolsResult = Awaited<ReturnType<McpClient['listTools']>>
