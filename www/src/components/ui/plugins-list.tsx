import { createElement, useEffect, useState } from 'react'
import axios from 'axios'
import { getApiBase } from '@/lib/config'
import { PluginListItem } from '@/components/ui/plugin-list-item'
import { pluginListHeader } from './plugin-list-header'
import { subscribe, AuthState } from '@/lib/auth'

type ToolItem = { name: string; description?: string; inputSchema?: any }
type OnSelectionChangeArg = { name: string; tools: ToolItem[] }
type PluginsListProps = { onSelectionChange?: (plugin: OnSelectionChangeArg) => void; onError?: (msg: string) => void }

export function pluginsList({ onSelectionChange, onError }: PluginsListProps = {}) {
    const [loading, setLoading] = useState(true)
    const [error, setError] = useState<string | null>(null)
    type ListItem = { id?: string; name: string; description?: string; tools?: ToolItem[]; owner?: string }
    const [items, setItems] = useState<ListItem[]>([])
    const [selectedName, setSelectedName] = useState<string | null>(null)
    const [auth, setAuth] = useState<AuthState>({ authenticated: false })

    useEffect(() => {
        const unsub = subscribe(setAuth)
        return unsub
    }, [])

    useEffect(() => {
        if (!auth.authenticated) {
            setLoading(false)
            setError(null)
            setItems([])
            return
        }
        let cancelled = false
        const toMsg = (e: any, fallback: string) => {
            const status = e?.response?.status
            const statusText = e?.response?.statusText
            const detail = e?.response?.data?.message || e?.message
            return [fallback, status ? `(${status}${statusText ? ` ${statusText}` : ''})` : '', detail ? `- ${detail}` : '']
                .filter(Boolean)
                .join(' ')
        }
        async function load() {
            try {
                setLoading(true)
                setError(null)
                const apiBase = getApiBase()
                const url = `${apiBase}/api/plugins`
                const res = await axios.get(url, {
                    headers: { 'Accept': 'application/json' },
                })
                if (import.meta.env.DEV) {
                    //console.debug('GET /api/plugins →', res.status, res.data)
                }
                const data = res.data
                if (cancelled) return

                // Normalize various shapes:
                // Preferred (fast path): object map where the key is the plugin name.
                // Example: { "hash": { name: "tools", tools: [...] }, "time": { ... } }
                let normalized: ListItem[] = []
                if (data && typeof data === 'object' && !Array.isArray(data)) {
                    const container = (data as any).plugins && typeof (data as any).plugins === 'object' && !Array.isArray((data as any).plugins)
                        ? (data as any).plugins
                        : data
                    if (container && typeof container === 'object' && !Array.isArray(container)) {
                        normalized = Object.entries(container as Record<string, any>).map(([key, val]) => {
                            const toolCount = Array.isArray((val as any)?.tools) ? (val as any).tools.length : 0
                            const desc = toolCount > 0 ? `${toolCount} tool${toolCount === 1 ? '' : 's'}` : ((val as any)?.description ?? 'no description')
                            const tools: ToolItem[] = Array.isArray((val as any)?.tools)
                                ? (val as any).tools.map((t: any) => ({
                                    name: String(t?.name ?? ''),
                                    description: typeof t?.description === 'string' ? t.description : undefined,
                                    inputSchema: (t as any)?.inputSchema,
                                })).filter((t: ToolItem) => t.name.length > 0)
                                : []
                            const owner = typeof (val as any)?.owner === 'string' ? (val as any).owner : undefined
                            return { name: key, description: desc, tools, owner }
                        })
                    }
                }

                // Fallbacks for array or other shapes
                if (normalized.length === 0) {
                    let rawList: any[] = []
                    if (Array.isArray(data)) {
                        rawList = data
                    } else if (Array.isArray((data as any)?.plugins)) {
                        rawList = (data as any).plugins
                    } else if (data && typeof data === 'object') {
                        const map = (data as any).plugins && typeof (data as any).plugins === 'object'
                            ? (data as any).plugins
                            : data
                        rawList = Object.keys(map).map((k) => ({ [k]: (map as any)[k] }))
                    }

                    normalized = rawList.map((entry) => {
                        if (typeof entry === 'string') {
                            return { name: entry, description: 'no description', tools: [] }
                        }
                        if (entry && typeof entry === 'object') {
                            const keys = Object.keys(entry)
                            if (keys.length === 1 && typeof keys[0] === 'string') {
                                const key = keys[0]
                                const val = (entry as any)[key]
                                const toolCount = Array.isArray(val?.tools) ? val.tools.length : 0
                                const desc = toolCount > 0 ? `${toolCount} tool${toolCount === 1 ? '' : 's'}` : (val?.description ?? 'no description')
                                const tools: ToolItem[] = Array.isArray(val?.tools)
                                    ? val.tools.map((t: any) => ({
                                        name: String(t?.name ?? ''),
                                        description: typeof t?.description === 'string' ? t.description : undefined,
                                        inputSchema: (t as any)?.inputSchema,
                                    })).filter((t: ToolItem) => t.name.length > 0)
                                    : []
                                const owner = typeof val?.owner === 'string' ? val.owner : undefined
                                return { name: key, description: desc, tools, owner }
                            }
                            const name = (entry as any).name ?? (entry as any).id ?? 'Untitled'
                            const description = (entry as any).description ?? 'no description'
                            const id = (entry as any).id
                            return { id, name, description, tools: [] }
                        }
                        return { name: 'Untitled', description: 'no description', tools: [] }
                    })
                }

                // Stable sort by name for nicer UX
                normalized.sort((a, b) => a.name.localeCompare(b.name))
                if (import.meta.env.DEV) {
                    //console.debug('Normalized plugins count:', normalized.length)
                }
                setItems(normalized)
            } catch (e: any) {
                if (import.meta.env.DEV) {
                    console.error('GET /api/plugins failed', e)
                }
                const apiBase = getApiBase()
                const url = `${apiBase}/api/plugins`
                const msg = `${toMsg(e, 'Failed to load plugins')} (url: ${url})`
                if (!cancelled) setError(msg)
                onError?.(msg)
            } finally {
                if (!cancelled) setLoading(false)
            }
        }
        load()
        return () => { cancelled = true }
    }, [auth.authenticated])

    async function refreshPlugins() {
        if (!auth.authenticated) return
        try {
            const apiBase = getApiBase()
            const url = `${apiBase}/api/plugins`
            const res = await axios.get(url, { headers: { 'Accept': 'application/json' } })
            const data = res.data
            // Reuse minimal normalization: prefer object map, else array forms
            let normalized: any[] = []
            if (data && typeof data === 'object' && !Array.isArray(data)) {
                const container = (data as any).plugins && typeof (data as any).plugins === 'object' && !Array.isArray((data as any).plugins)
                    ? (data as any).plugins
                    : data
                if (container && typeof container === 'object' && !Array.isArray(container)) {
                    normalized = Object.entries(container as Record<string, any>).map(([key, val]) => {
                        const toolCount = Array.isArray((val as any)?.tools) ? (val as any).tools.length : 0
                        const desc = toolCount > 0 ? `${toolCount} tool${toolCount === 1 ? '' : 's'}` : ((val as any)?.description ?? 'no description')
                        const tools: ToolItem[] = Array.isArray((val as any)?.tools)
                            ? (val as any).tools.map((t: any) => ({
                                name: String(t?.name ?? ''),
                                description: typeof t?.description === 'string' ? t.description : undefined,
                                inputSchema: (t as any)?.inputSchema,
                            })).filter((t: ToolItem) => t.name.length > 0)
                            : []
                        const owner = typeof (val as any)?.owner === 'string' ? (val as any).owner : undefined
                        return { name: key, description: desc, tools, owner }
                    })
                }
            }
            if (normalized.length === 0) {
                let rawList: any[] = []
                if (Array.isArray(data)) rawList = data
                else if (Array.isArray((data as any)?.plugins)) rawList = (data as any).plugins
                else if (data && typeof data === 'object') {
                    const map = (data as any).plugins && typeof (data as any).plugins === 'object' ? (data as any).plugins : data
                    rawList = Object.keys(map).map((k) => ({ [k]: (map as any)[k] }))
                }
                normalized = rawList.map((entry) => {
                    if (typeof entry === 'string') return { name: entry, description: 'no description', tools: [] }
                    if (entry && typeof entry === 'object') {
                        const keys = Object.keys(entry)
                        if (keys.length === 1 && typeof keys[0] === 'string') {
                            const key = keys[0]
                            const val = (entry as any)[key]
                            const toolCount = Array.isArray(val?.tools) ? val.tools.length : 0
                            const desc = toolCount > 0 ? `${toolCount} tool${toolCount === 1 ? '' : 's'}` : (val?.description ?? 'no description')
                            const tools: ToolItem[] = Array.isArray(val?.tools)
                                ? val.tools.map((t: any) => ({
                                    name: String(t?.name ?? ''),
                                    description: typeof t?.description === 'string' ? t.description : undefined,
                                    inputSchema: (t as any)?.inputSchema,
                                })).filter((t: ToolItem) => t.name.length > 0)
                                : []
                            const owner = typeof val?.owner === 'string' ? val.owner : undefined
                            return { name: key, description: desc, tools, owner }
                        }
                        const name = (entry as any).name ?? (entry as any).id ?? 'Untitled'
                        const description = (entry as any).description ?? 'no description'
                        const id = (entry as any).id
                        return { id, name, description, tools: [] }
                    }
                    return { name: 'Untitled', description: 'no description', tools: [] }
                })
            }
            normalized.sort((a, b) => a.name.localeCompare(b.name))
            setItems(normalized)
        } catch (e) {
            if (import.meta.env.DEV) console.error('GET /api/plugins failed on refresh', e)
        }
    }

    async function handleDelete(name: string) {
        try {
            const apiBase = getApiBase()
            const url = `${apiBase}/api/plugins/${encodeURIComponent(name)}`
            await axios.delete(url)
        } catch (e) {
            if (import.meta.env.DEV) console.error('DELETE /api/plugins failed', e)
            const err: any = e
            const status = err?.response?.status
            const statusText = err?.response?.statusText
            const detail = err?.response?.data?.message || err?.message
            const apiBase = getApiBase()
            const url = `${apiBase}/api/plugins/${encodeURIComponent(name)}`
            const msg = [
                `Failed to delete plugin "${name}"`,
                status ? `(${status}${statusText ? ` ${statusText}` : ''})` : '',
                detail ? `- ${detail}` : '',
                `(url: ${url})`
            ]
                .filter(Boolean)
                .join(' ')
            onError?.(msg)
        }
        // Refresh list
        try {
            const apiBase = getApiBase()
            const url = `${apiBase}/api/plugins`
            const res = await axios.get(url, { headers: { 'Accept': 'application/json' } })
            const data = res.data
            // Reuse normalization logic by mimicking load, but inline to avoid refactor
            let normalized: ListItem[] = []
            if (data && typeof data === 'object' && !Array.isArray(data)) {
                const container = (data as any).plugins && typeof (data as any).plugins === 'object' && !Array.isArray((data as any).plugins)
                    ? (data as any).plugins
                    : data
                if (container && typeof container === 'object' && !Array.isArray(container)) {
                    normalized = Object.entries(container as Record<string, any>).map(([key, val]) => {
                        const toolCount = Array.isArray((val as any)?.tools) ? (val as any).tools.length : 0
                        const desc = toolCount > 0 ? `${toolCount} tool${toolCount === 1 ? '' : 's'}` : ((val as any)?.description ?? 'no description')
                        const tools: ToolItem[] = Array.isArray((val as any)?.tools)
                            ? (val as any).tools.map((t: any) => ({
                                name: String(t?.name ?? ''),
                                description: typeof t?.description === 'string' ? t.description : undefined,
                                inputSchema: (t as any)?.inputSchema,
                            })).filter((t: ToolItem) => t.name.length > 0)
                            : []
                        const owner = typeof (val as any)?.owner === 'string' ? (val as any).owner : undefined
                        return { name: key, description: desc, tools, owner }
                    })
                }
            }
            if (normalized.length === 0) {
                let rawList: any[] = []
                if (Array.isArray(data)) {
                    rawList = data
                } else if (Array.isArray((data as any)?.plugins)) {
                    rawList = (data as any).plugins
                } else if (data && typeof data === 'object') {
                    const map = (data as any).plugins && typeof (data as any).plugins === 'object'
                        ? (data as any).plugins
                        : data
                    rawList = Object.keys(map).map((k) => ({ [k]: (map as any)[k] }))
                }
                normalized = rawList.map((entry) => {
                    if (typeof entry === 'string') {
                        return { name: entry, description: 'no description', tools: [] }
                    }
                    if (entry && typeof entry === 'object') {
                        const keys = Object.keys(entry)
                        if (keys.length === 1 && typeof keys[0] === 'string') {
                            const key = keys[0]
                            const val = (entry as any)[key]
                            const toolCount = Array.isArray(val?.tools) ? val.tools.length : 0
                            const desc = toolCount > 0 ? `${toolCount} tool${toolCount === 1 ? '' : 's'}` : (val?.description ?? 'no description')
                            const tools: ToolItem[] = Array.isArray(val?.tools)
                                ? val.tools.map((t: any) => ({
                                    name: String(t?.name ?? ''),
                                    description: typeof t?.description === 'string' ? t.description : undefined,
                                    inputSchema: (t as any)?.inputSchema,
                                })).filter((t: ToolItem) => t.name.length > 0)
                                : []
                            const owner = typeof val?.owner === 'string' ? val.owner : undefined
                            return { name: key, description: desc, tools, owner }
                        }
                        const name = (entry as any).name ?? (entry as any).id ?? 'Untitled'
                        const description = (entry as any).description ?? 'no description'
                        const id = (entry as any).id
                        return { id, name, description, tools: [] }
                    }
                    return { name: 'Untitled', description: 'no description', tools: [] }
                })
            }
            normalized.sort((a, b) => a.name.localeCompare(b.name))
            setItems(normalized)
        } catch (e) {
            if (import.meta.env.DEV) console.error('GET /api/plugins failed after delete', e)
            const err: any = e
            const status = err?.response?.status
            const statusText = err?.response?.statusText
            const detail = err?.response?.data?.message || err?.message
            const apiBase = getApiBase()
            const url = `${apiBase}/api/plugins`
            const msg = [
                'Failed to refresh plugins after delete',
                status ? `(${status}${statusText ? ` ${statusText}` : ''})` : '',
                detail ? `- ${detail}` : '',
                `(url: ${url})`
            ]
                .filter(Boolean)
                .join(' ')
            onError?.(msg)
        }
    }

    return (
        <aside id="pluginsList" className="w-[300px] ark-plugins-list  shrink-0 rounded-md border p-6 ark-border-dimmed ">
            {createElement(pluginListHeader, { onError, onRefresh: refreshPlugins, auth })}
            <div className="mt-3">
                {loading && (
                    <div className="text-xs text-muted-foreground">Loading…</div>
                )}
                {error && (
                    <div className="text-xs text-destructive">{error}</div>
                )}
                {!loading && !error && items.length === 0 && (
                    <div className="text-xs text-muted-foreground">No plugins loaded.</div>
                )}
                {!loading && !error && items.length > 0 && (
                    <ul className="space-y-1 text-sm" role="listbox" aria-label="Plugins">
                        {items.map((p, idx) => (
                            <PluginListItem
                                key={String(p.id ?? p.name ?? idx)}
                                name={p.name}
                                description={p.description}
                                owner={p.owner}
                                selected={selectedName === p.name}
                                onSelect={() => {
                                    setSelectedName(p.name)
                                    const tools = p.tools ?? []
                                    onSelectionChange?.({ name: p.name, tools })
                                }}
                                onDelete={handleDelete}
                            />
                        ))}
                    </ul>
                )}
            </div>
        </aside>
    )
}
