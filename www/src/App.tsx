import { createElement, useEffect, useState } from 'react'

import './App.css'
import { Footer } from '@/components/ui/footer'
import { pluginsList } from '@/components/ui/plugins-list'
import { ToolsHeader } from '@/components/ui/main-panel-header'
import { ToolsListItem } from '@/components/ui/tools-list-item'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { ChartNoAxesCombined, Box } from "lucide-react"
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card'
import { TitleBar } from '@/components/ui/title-bar'
import { AlertBanner } from '@/components/ui/alert-banner'
import { getMcp, type McpToolResult } from '@/lib/mcp-client'
import { getMcpBase } from '@/lib/config'
import { HealthReadinessCard } from '@/components/ui/metrics/HealthReadinessCard'
import { McpRequestsCard } from '@/components/ui/metrics/McpRequestsCard'
import { AvgLatencyCard } from '@/components/ui/metrics/AvgLatencyCard'
import { ThroughputCard } from '@/components/ui/metrics/ThroughputCard'
import { ToolCallsCard } from '@/components/ui/metrics/ToolCallsCard'
import { ToolLatencyCard } from '@/components/ui/metrics/ToolLatencyCard'
import { LatencyOverTimeCard } from '@/components/ui/metrics/LatencyOverTimeCard'
import { subscribe, AuthState } from '@/lib/auth'

type ToolItem = { name: string; description?: string; inputSchema?: any }
type TransportMode = 'streamable-http' | 'sse'

export default function App() {
  const [selectedPlugin, setSelectedPlugin] = useState<{ name: string; tools: ToolItem[] } | null>(null)
  const [selectedToolName, setSelectedToolName] = useState<string | null>(null)
  const [errorMessage, setErrorMessage] = useState<string | null>(null)
  const [auth, setAuth] = useState<AuthState>({ authenticated: false })
  const [transport, setTransport] = useState<TransportMode>('streamable-http')
  // Output is shown only inside the tool dialog now

  // Persist active tab across refresh
  const TAB_STORAGE_KEY = 'ark.ui.activeTab'
  const allowedTabs = new Set(['content', 'metrics'])
  const [activeTab, setActiveTab] = useState<string>(() => {
    try {
      const v = localStorage.getItem(TAB_STORAGE_KEY) || 'content'
      return allowedTabs.has(v) ? v : 'content'
    } catch {
      return 'content'
    }
  })
  useEffect(() => {
    try { localStorage.setItem(TAB_STORAGE_KEY, activeTab) } catch { }
  }, [activeTab])

  // Reset selected tool when plugin changes
  useEffect(() => {
    setSelectedToolName(null)
  }, [selectedPlugin?.name])

  useEffect(() => {
    return subscribe(setAuth)
  }, [])

  // Read transport from URL params
  useEffect(() => {
    const params = new URLSearchParams(window.location.search)
    const transportParam = params.get('transport')
    if (transportParam === 'sse') {
      setTransport('sse')
    } else {
      setTransport('streamable-http')
    }
  }, [])

  async function runTool(toolName: string, args?: Record<string, any>): Promise<string | undefined> {
    if (!auth.authenticated) {
      setErrorMessage('Authentication required to run tools')
      return undefined
    }
    try {
      const base = `${getMcpBase()}/mcp`
      const client = getMcp(base, transport)
      const result: McpToolResult = await client.callTool(toolName, args)
      const parts = Array.isArray((result as any)?.content) ? (result as any).content : []
      const text = parts
        .filter((p: any) => p && typeof p === 'object' && p.type === 'text' && typeof p.text === 'string')
        .map((p: any) => p.text)
        .join('\n')
      return text || 'Tool executed.'
    } catch (e: any) {
      if (import.meta.env.DEV) console.error('MCP callTool failed', e)
      const status = e?.response?.status
      const statusText = e?.response?.statusText
      const detail = e?.response?.data?.message || e?.message
      const msg = [
        `Failed to run tool "${toolName}"`,
        status ? `(${status}${statusText ? ` ${statusText}` : ''})` : '',
        detail ? `- ${detail}` : '',
        `(url: ${getMcpBase()}/mcp)`
      ].filter(Boolean).join(' ')
      setErrorMessage(msg)
      return undefined
    }
  }

  return (
    <div className="min-h-screen bg-background text-foreground relative flex flex-col">
      {/* Title Bar */}
      <TitleBar />

      {/* Tabs wrapping Content (plugins/tools), with disabled Metrics and Settings */}
      <div className="mx-auto max-w-screen-2xl px-4 py-6 w-full flex-1">
        <Tabs value={activeTab} onValueChange={(v) => setActiveTab(v)}>
          <TabsList className="mb-4">
            <TabsTrigger value="content"><Box className="ark-tab-icon" /> Content</TabsTrigger>
            <TabsTrigger value="metrics" disabled={!auth.user?.is_admin}><ChartNoAxesCombined className="ark-tab-icon" /> Stats</TabsTrigger>
          </TabsList>
          <TabsContent value="content" className="m-0">
            {errorMessage ? (
              <AlertBanner variant="destructive" onClose={() => setErrorMessage(null)}>
                {errorMessage}
              </AlertBanner>
            ) : null}
            <div className="flex items-stretch gap-6 text-card-foreground ">
              {/* Left column: pluginsList (300px) */}
              {createElement(pluginsList, { onSelectionChange: setSelectedPlugin, onError: (msg: string) => setErrorMessage(msg) })}

              {/* Right column: auto-sized main area */}
              <main className="flex-1 rounded-md border p-6 text-card-foreground min-h-[400px] ark-border-dimmed">
                <div className="flex items-center justify-between ">
                  <ToolsHeader
                    title={selectedPlugin ? (
                      <>
                        Tools in <span className="font-semibold">{selectedPlugin.name}</span> plugin
                      </>
                    ) : (
                      "Tools"
                    )}
                    subtitle={selectedPlugin ? (
                      <>There {selectedPlugin.tools.length === 1 ? 'is' : 'are'} {selectedPlugin.tools.length} {selectedPlugin.tools.length === 1 ? 'tool' : 'tools'} in the plugin</>
                    ) : undefined}
                  />
                </div>
                <div className="mt-4 ">
                  {selectedPlugin ? (
                    <>
                      {selectedPlugin.tools.length > 0 ? (
                        <ul className="space-y-1 text-sm " role="listbox" aria-label="Tools">
                          {selectedPlugin.tools.map((t, i) => (
                            <ToolsListItem
                              key={i}
                              name={t.name}
                              description={t.description}
                              selected={selectedToolName === t.name}
                              onSelect={() => setSelectedToolName(t.name)}
                              inputSchema={t.inputSchema}
                              onRun={async (args) => {
                                setSelectedToolName(t.name)
                                return await runTool(t.name, args)
                              }}
                            />
                          ))}
                        </ul>
                      ) : (
                        <div className="text-xs text-muted-foreground">No tools available.</div>
                      )}
                    </>
                  ) : (
                    <div className="text-xs text-muted-foreground">Select a plugin to see its tools.</div>
                  )}
                </div>
              </main>
            </div>
          </TabsContent>

          <TabsContent value="metrics" className="m-0">
            <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4">
              <HealthReadinessCard />
              <McpRequestsCard />
              <AvgLatencyCard />
              <ThroughputCard />
            </div>
            <div className="mt-4 grid grid-cols-1 gap-4">
              <LatencyOverTimeCard />
            </div>
            <div className="mt-4 grid grid-cols-1 lg:grid-cols-4 gap-4 ark-metrics-card ark-metrics-card-2">
              <ToolCallsCard className="lg:col-span-2" />
              <ToolLatencyCard className="lg:col-span-2 ark-metrics-card ark-metrics-card-2" />
            </div>
          </TabsContent>
        </Tabs>
      </div>

      <Footer />
    </div>
  )
}
