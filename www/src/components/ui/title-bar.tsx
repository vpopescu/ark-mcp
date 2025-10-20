import { ThemeToggle } from '@/components/ui/theme-toggle'
import { LoginButton } from '@/components/ui/login-button'
import mcpLogo from '@/assets/img/mcp.svg'

export function TitleBar() {
    return (
        <div className="bg-background text-foreground ark-title-font ">
            <div className="relative flex flex-col items-start max-w-screen-2xl px-4 py-3 mx-auto">
                <div className="absolute top-2 right-4 flex items-center gap-2">
                    <LoginButton />
                    <ThemeToggle />
                </div>
                <div className="flex items-center">
                    <h1 className="ark-title ark-title-font tracking-wide">ARK ···</h1>
                </div>
                <div className="flex ark-subtitle ark-title-font items-center mt-2 text-sm text-muted-foreground  ">
                    model · context · protocol  server
                </div>
            </div>
        </div>
    )
}
