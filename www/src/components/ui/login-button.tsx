import { useEffect, useState } from 'react'
import { User } from 'lucide-react'
import { subscribe, refreshStatus, logout, ensureLogin, AuthState, initialStatusCheck } from '@/lib/auth'
import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar'
import { Badge } from '@/components/ui/badge'
import {
    DropdownMenu,
    DropdownMenuContent,
    DropdownMenuItem,
    DropdownMenuSeparator,
    DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'

export function LoginButton() {
    const [auth, setAuth] = useState<AuthState>({ authenticated: false })
    useEffect(() => {
        const unsub = subscribe(setAuth)
        initialStatusCheck('')
        return unsub
    }, [])

    // Helper function to get user initials for avatar fallback
    const getUserInitials = (name?: string, email?: string) => {
        if (name) {
            return name.split(' ').map(n => n[0]).join('').toUpperCase().slice(0, 2)
        }
        if (email) {
            return email[0].toUpperCase()
        }
        return 'U'
    }

    // Helper function to get avatar URL from IDP or fall back to initials
    const getAvatarUrl = (user?: { email?: string; picture?: string }) => {
        // Use IDP-provided picture if available
        if (user?.picture) {
            return user.picture
        }
        // Fall back to initials (no external services)
        return undefined
    }

    // When auth is disabled, don't show any login/user UI
    if (auth.auth_disabled) {
        return null
    }

    return (
        <>
            {auth.authenticated ? (
                <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                        <button
                            className="rounded-md border ark-border-dimmed hover:bg-accent transition-colors p-1"
                            aria-label="User menu"
                            title="User account menu"
                        >
                            <Avatar className="h-6 w-6">
                                <AvatarImage
                                    src={getAvatarUrl(auth.user)}
                                    alt={auth.user?.name || auth.user?.email || 'User'}
                                />
                                <AvatarFallback className="bg-primary text-primary-foreground text-xs">
                                    {getUserInitials(auth.user?.name, auth.user?.email)}
                                </AvatarFallback>
                            </Avatar>
                        </button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end" className="w-64">
                        {/* User Card */}
                        <div className="flex items-center gap-3 p-3">
                            <div className="flex flex-col items-center gap-1">
                                <Avatar className="h-10 w-10">
                                    <AvatarImage
                                        src={getAvatarUrl(auth.user)}
                                        alt={auth.user?.name || auth.user?.email || 'User'}
                                    />
                                    <AvatarFallback className="bg-primary text-primary-foreground">
                                        {getUserInitials(auth.user?.name, auth.user?.email)}
                                    </AvatarFallback>
                                </Avatar>
                                {auth.user?.is_admin && (
                                    <Badge
                                        variant="secondary"
                                        className="admin-badge"
                                    >
                                        admin
                                    </Badge>
                                )}
                            </div>
                            <div className="flex flex-col min-w-0 flex-1">
                                {auth.user?.name && (
                                    <div className="font-medium text-sm truncate">
                                        {auth.user.name}
                                    </div>
                                )}
                                {auth.user?.email && (
                                    <div className="text-xs text-muted-foreground truncate">
                                        {auth.user.email}
                                    </div>
                                )}
                                {!auth.user?.name && !auth.user?.email && (
                                    <div className="text-sm text-muted-foreground">
                                        {auth.user?.subject || 'Unknown User'}
                                    </div>
                                )}
                            </div>
                        </div>

                        <DropdownMenuSeparator />

                        <DropdownMenuItem
                            onClick={() => logout('').then(() => window.location.href = '/admin')}
                            className="cursor-pointer"
                        >
                            Sign out
                        </DropdownMenuItem>
                    </DropdownMenuContent>
                </DropdownMenu>
            ) : (
                <button
                    onClick={() => ensureLogin('')}
                    className="text-xs px-2 py-1 rounded-md border ark-border-dimmed hover:bg-accent transition-colors"
                    aria-label="Login"
                    title="Sign in with Microsoft Entra ID"
                >
                    log in
                </button>
            )}
        </>
    )
}