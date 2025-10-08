import { useEffect, useState } from 'react'
import { User } from 'lucide-react'
import { subscribe, refreshStatus, logout, ensureLogin, AuthState, initialStatusCheck } from '@/lib/auth'
import { Avatar, AvatarFallback } from '@/components/ui/avatar'
import {
    DropdownMenu,
    DropdownMenuContent,
    DropdownMenuItem,
    DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'

export function LoginButton() {
    const [auth, setAuth] = useState<AuthState>({ authenticated: false })
    useEffect(() => {
        const unsub = subscribe(setAuth)
        initialStatusCheck('')
        return unsub
    }, [])

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
                                <AvatarFallback className="bg-primary text-primary-foreground">
                                    <User className="h-4 w-4" />
                                </AvatarFallback>
                            </Avatar>
                        </button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end">
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