function ensureProtocol(hostOrUrl: string): string {
    if (!hostOrUrl) return ''
    if (/^https?:\/\//i.test(hostOrUrl)) return hostOrUrl
    const proto = (typeof window !== 'undefined' && window.location?.protocol) || 'http:'
    return `${proto}//${hostOrUrl}`
}

export function getApiBase(): string {
    // Optional UI override from Settings
    try {
        if (typeof window !== 'undefined') {
            const raw = localStorage.getItem('ark.ui.settings')
            if (raw) {
                const parsed = JSON.parse(raw) as { apiBase?: string }
                const v = (parsed?.apiBase || '').trim()
                if (v) return ensureProtocol(v)
            }
        }
    } catch {}
    const envObj = (import.meta as any)?.env || {}
    // Prefer direct token access so Vite define() replacements apply
    const directVite = (import.meta as any).env.VITE_ARK_SERVER_API as string | undefined
    const directRaw = (import.meta as any).env.ARK_SERVER_API as string | undefined
    // Fallback to object indirection (will only include VITE_* by default)
    const indirect = (envObj.VITE_ARK_SERVER_API as string | undefined) || (envObj.ARK_SERVER_API as string | undefined)
    const fromEnv = directVite || directRaw || indirect

    if (fromEnv && fromEnv.trim().length > 0) return ensureProtocol(fromEnv.trim())
    // default: same as React app origin
    if (typeof window !== 'undefined') {
        const { origin } = window.location
        return origin
    }
    return 'http://localhost:8000'
}

export function getMcpBase(): string {
    // Optional UI override from Settings
    try {
        if (typeof window !== 'undefined') {
            const raw = localStorage.getItem('ark.ui.settings')
            if (raw) {
                const parsed = JSON.parse(raw) as { mcpBase?: string }
                const v = (parsed?.mcpBase || '').trim()
                if (v) return ensureProtocol(v)
            }
        }
    } catch {}
    const envObj = (import.meta as any)?.env || {}
    const directVite = (import.meta as any).env.VITE_ARK_SERVER_MCP as string | undefined
    const directRaw = (import.meta as any).env.ARK_SERVER_MCP as string | undefined
    const indirect = (envObj.VITE_ARK_SERVER_MCP as string | undefined) || (envObj.ARK_SERVER_MCP as string | undefined)
    const fromEnv = directVite || directRaw || indirect
    if (fromEnv && fromEnv.trim().length > 0) return ensureProtocol(fromEnv.trim())
    // default: same hostname as app, port 3001
    if (typeof window !== 'undefined') {
        const { protocol, hostname } = window.location
        return `${protocol}//${hostname}:3001`
    }
    return 'http://localhost:3001'
}



function joinUrl(base: string, path: string): string {
    if (!path) return base
    if (/^https?:\/\//i.test(path)) return path
    const b = base.replace(/\/?$/, '')
    const p = path.startsWith('/') ? path : `/${path}`
    return `${b}${p}`
}


// On load, log any provided env overrides for visibility
(() => {

    const envObj = (import.meta as any)?.env || {}
    const envApi = (envObj.VITE_ARK_SERVER_API as string | undefined) || (envObj.ARK_SERVER_API as string | undefined)
    const envMcp = (envObj.VITE_ARK_SERVER_MCP as string | undefined) || (envObj.ARK_SERVER_MCP as string | undefined)
    if (envApi && envApi.trim()) console.log('ARK_SERVER_API =', envApi.trim())
    if (envMcp && envMcp.trim()) console.log('ARK_SERVER_MCP =', envMcp.trim())
    // In some setups MODE/DEV may not be present on envObj due to define-time replacement.
    // Treat any non-production (or missing flags) as dev-like for visibility.
    const isDevLike = !!(envObj && (envObj.DEV || envObj.MODE !== 'production'))
    if (typeof window !== 'undefined' && isDevLike) {
        try {
            console.debug('Resolved API base:', getApiBase())
            console.debug('Resolved MCP base:', getMcpBase())
        } catch { }
    }
})()
