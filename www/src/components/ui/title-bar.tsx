import { ThemeToggle } from '@/components/ui/theme-toggle'
import mcpLogo from '@/assets/img/mcp.svg'

export function TitleBar() {
    return (
        <div className="bg-background text-foreground ark-title-font ">


            <div className="relative flex flex-col items-start max-w-screen-2xl px-4 py-3 mx-auto">
                {/* Theme button fixed at top-right with 10px padding */}
                <ThemeToggle />
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
