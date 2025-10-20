import { Moon, Sun, SunMoon } from "lucide-react";
import { useThemePreference } from "@/lib/theme"
import {
  HoverCard, HoverCardContent,
  HoverCardTrigger
} from "@radix-ui/react-hover-card";
import { Button } from '@/components/ui/button';


export function ThemeToggle() {
  const { mode, setMode, cycle } = useThemePreference()

  const Icon = mode === 'system' ? SunMoon : mode === 'light' ? Sun : Moon

  return (

    <HoverCard openDelay={0} closeDelay={100}>
      <HoverCardTrigger asChild>
        <Button
          type="button"
          aria-label={`Toggle theme (current: ${mode})`}
          title="Switch color theme"
          onClick={() => setMode((m) => cycle.next(m))}
          className="ark-action-btn grid place-items-center rounded-md border bg-card p-2 text-card-foreground shadow hover:bg-accent hover:text-accent-foreground"
        >
          <Icon className="h-4 w-4" />
        </Button>
      </HoverCardTrigger>
      <HoverCardContent>
        <span className="text-xs text-muted-foreground">
          {mode === 'dark' && 'Dark theme'}
          {mode === 'light' && 'Light theme'}
          {mode === 'system' && 'Match operating system'}
        </span>
      </HoverCardContent>


    </HoverCard>
  )
}
