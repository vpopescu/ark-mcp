import type { KeyboardEvent } from 'react'
import { Trash, User, Users } from 'lucide-react'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'

import { Button } from '@/components/ui/button'

export type PluginLike = string | { id?: string; name?: string; description?: string;[key: string]: unknown }

export type PluginListItemProps = {
    name: string
    description?: string
    owner?: string
    selected?: boolean
    onSelect?: () => void
    onDelete?: (name: string) => void
}

export function PluginListItem({ name, description, owner, selected = false, onSelect, onDelete }: PluginListItemProps) {
    const handleKeyDown = (e: KeyboardEvent<HTMLLIElement>) => {
        if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault()
            onSelect?.()
        }
    }
    return (
        <li
            role="option"
            aria-selected={selected}
            tabIndex={0}
            onClick={onSelect}
            onKeyDown={handleKeyDown}
            className={
                'ark-plugin-entry ark-border-dimmed ark-plugin-item rounded-md border px-2 py-1 pr-8 text-left cursor-pointer outline-none relative ' +
                (selected ? 'bg-accent/20 text-accent-foreground' :
                    'hover:bg-accent/60 hover:text-accent-foreground')
            }
        >
            <TooltipProvider>
                <div className="flex items-center gap-2">
                    {owner === '*/*/*' || !owner ? (
                        <Tooltip>
                            <TooltipTrigger asChild>
                                <Users className="h-4 w-4 text-muted-foreground" />
                            </TooltipTrigger>
                            <TooltipContent>
                                <p>Shared plugin</p>
                            </TooltipContent>
                        </Tooltip>
                    ) : (
                        <Tooltip>
                            <TooltipTrigger asChild>
                                <User className="h-4 w-4 text-muted-foreground" />
                            </TooltipTrigger>
                            <TooltipContent>
                                <p>Private plugin</p>
                            </TooltipContent>
                        </Tooltip>
                    )}
                    <div className="text-sm text-primary">{name}</div>
                </div>
            </TooltipProvider>
            {description && (
                <div className="text-xs text-muted-foreground ml-6">{description}</div>
            )}
            <Button
                variant="ghost"
                className="ark-action-btn ark-ghost ark-delete absolute right-1 top-1/2 -translate-y-1/2"
                aria-label="Delete plugin"
                title="Delete"
                onClick={(e) => {
                    e.stopPropagation()
                    onDelete?.(name)
                }}
            >
                <Trash className="h-4 w-4" />
            </Button>
        </li>
    )
}
