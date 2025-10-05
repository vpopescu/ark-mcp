import { Input } from "@/components/ui/input"
import { Filter  } from "lucide-react"
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover"
import { Button } from "@/components/ui/button"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"

export function ToolsHeader({ title = "Main panel", subtitle }: { title?: React.ReactNode; subtitle?: React.ReactNode }) {
  return (
  <div className="ark-tools-header flex w-full items-center gap-3">
      <div className="flex min-w-0 flex-col">
        <div className="ark-section-title leading-tight">{title}</div>
        {subtitle ? (
          <div className="text-xs text-muted-foreground mt-0.5 truncate">{subtitle}</div>
        ) : null}
      </div>

      <div className="ml-auto ark-section-filter flex items-center gap-2 justify-end">
       

  <Popover >
      <PopoverTrigger asChild>
    <Button variant="outline" className="ark-ghost" hidden={true}>
              <Filter className="h-4 w-4" />
            </Button>
          </PopoverTrigger>
          <PopoverContent className="w-[400px]" onFocusOutside={(e) => e.preventDefault()}>
            <div className="ark-field-label">Filter</div>
            <Input
              
              type="text"
              placeholder="Filter content"
              aria-label="Filter main panel content"
              className="h-9 w-full rounded-md bg-background px-3 text-sm placeholder:text-muted-foreground"
            />
          </PopoverContent>
        </Popover>
      </div>
    </div>
  )
}
