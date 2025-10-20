// Simplified auth helper: uses server-side OIDC session flow.
// Auto-start login when a protected fetch returns 401 with { auth: 'required' }.
// Server handles PKCE and token exchange internally.

export interface AuthUser { subject: string; email?: string; name?: string; picture?: string; provider: string; roles?: string[]; is_admin?: boolean }
export interface AuthState { authenticated: boolean; user?: AuthUser; auth_disabled?: boolean }

let current: AuthState = { authenticated: false }
let listeners: Array<(s: AuthState) => void> = []

function notify() { listeners.forEach(l => l(current)) }

export function subscribe(listener: (s: AuthState) => void) {
    listeners.push(listener)
    listener(current)
    return () => { listeners = listeners.filter(l => l !== listener) }
}

export async function refreshStatus(base = '') {
    // Don't auto-trigger login on callback page to prevent infinite loops
    const isCallbackPage = window.location.pathname === '/auth/callback'

    try {
        const res = await fetch(`${base}/auth/status`, { credentials: 'include' })
        if (res.ok) {
            const data = await res.json()
            if (data.user) {
                current = { authenticated: true, user: data.user, auth_disabled: data.auth_disabled }
            } else {
                current = { authenticated: false, auth_disabled: data.auth_disabled }
            }
        } else if (res.status === 401 && !isCallbackPage) {
            // Check if auth is required and trigger login (but not on callback page)
            try {
                const data = await res.json()
                if (data && data.auth === 'required') {
                    current = { authenticated: false }
                    notify() // Notify first
                    await ensureLogin(base) // Then attempt login
                    return
                }
            } catch {/* ignore */ }
            current = { authenticated: false }
        } else {
            current = { authenticated: false }
        }
    } catch {
        current = { authenticated: false }
    }
    notify()
}

// Initial status check without auto-login trigger
export async function initialStatusCheck(base = '') {
    try {
        const res = await fetch(`${base}/auth/status`, { credentials: 'include' })
        if (res.ok) {
            const data = await res.json()
            if (data.user) {
                current = { authenticated: true, user: data.user, auth_disabled: data.auth_disabled }
            } else {
                current = { authenticated: false, auth_disabled: data.auth_disabled }
            }
        } else {
            current = { authenticated: false }
        }
    } catch {
        current = { authenticated: false }
    }
    notify()
}

export async function logout(base = '') {
    try {
        const res = await fetch(`${base}/auth/logout`, { credentials: 'include' })
        if (res.ok) {
            current = { authenticated: false }
        } else {
            console.warn('Logout failed:', res.status)
            // Still set to unauthenticated locally
            current = { authenticated: false }
        }
    } catch (e) {
        console.warn('Logout request failed:', e)
        current = { authenticated: false }
    }
    notify()
}

export async function ensureLogin(base = ''): Promise<void> {
    if (current.authenticated) return
    try {
        // Server handles PKCE and provider discovery internally
        const res = await fetch(`${base}/auth/login`, { credentials: 'include' })
        if (!res.ok) return

        const ct = res.headers.get('content-type') || ''
        if (ct.includes('application/json')) {
            const data = await res.json().catch(() => null as any)
            if (data?.redirect) {
                window.location.href = data.redirect
            }
        }
    } catch (e) {
        console.warn('login start failed', e)
    }
}

export async function protectedFetch(input: RequestInfo | URL, init?: RequestInit & { base?: string }) {
    const res = await fetch(input, { ...init, credentials: 'include' })
    if (res.status === 401) {
        try {
            const data = await res.clone().json()
            if (data && data.auth === 'required') {
                await ensureLogin(init?.base)
            }
        } catch {/* ignore */ }
    }
    return res
}

// Kick initial status poll (without auto-login)
initialStatusCheck('')
