import { useMemo, useState, type KeyboardEvent } from 'react'
import { Play, Copy, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Checkbox } from '@/components/ui/checkbox'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'

export type ToolsListItemProps = {
    name: string
    description?: string
    selected?: boolean
    onSelect?: () => void
    onRun?: (args?: Record<string, any>) => Promise<string | undefined | null> | void
    inputSchema?: any
}

export function ToolsListItem({ name, description, selected = false, onSelect, onRun, inputSchema }: ToolsListItemProps) {
    const handleKeyDown = (e: KeyboardEvent<HTMLLIElement>) => {
        if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault()
            onSelect?.()
        }
    }
    const initialValues = useMemo(() => {
        const values: Record<string, any> = {}
        const props = inputSchema?.properties ?? {}
        for (const key of Object.keys(props)) {
            const p = props[key] ?? {}
            if (p.type === 'boolean') values[key] = Boolean(p.default ?? false)
            else values[key] = p.default ?? ''
        }
        return values
    }, [inputSchema])
    const [formValues, setFormValues] = useState<Record<string, any>>(initialValues)
    const [results, setResults] = useState<string[]>([])
    const [open, setOpen] = useState(false)

    const prettifyIfJson = (val: any): string | undefined => {
        if (val == null) return undefined
        if (typeof val === 'string') {
            const t = val.trim()
            if ((t.startsWith('{') && t.endsWith('}')) || (t.startsWith('[') && t.endsWith(']'))) {
                try { return JSON.stringify(JSON.parse(t), null, 2) } catch { /* not JSON */ }
            }
            return val
        }
        try { return JSON.stringify(val, null, 2) } catch { return String(val) }
    }

    const coerceArgs = () => {
        const args: Record<string, any> = {}
        const props = inputSchema?.properties ?? {}
        for (const key of Object.keys(props)) {
            const p = props[key] ?? {}
            const t = p.type ?? 'string'
            const v = formValues[key]
            if (t === 'integer' || t === 'number') {
                const n = typeof v === 'number' ? v : v === '' ? undefined : Number(v)
                if (typeof n === 'number' && !Number.isNaN(n)) args[key] = n
            } else if (t === 'boolean') {
                args[key] = Boolean(v)
            } else {
                args[key] = v ?? ''
            }
        }
        return args
    }
    return (
        <li
            role="option"
            aria-selected={selected}
            tabIndex={0}
            onClick={onSelect}
            onKeyDown={handleKeyDown}
            className={
                'ark-plugin-entry ark-plugin-item rounded-md border ark-border-dimmed px-2 py-1 pr-8 text-left cursor-pointer outline-none relative ' +
                (selected ? 'bg-accent/30 text-accent-foreground'
                    : 'hover:bg-accent/60 hover:text-accent-foreground')
            }
        >
            <div className="text-sm text-primary">{name}</div>
            {description && (
                <div className="text-xs text-muted-foreground">{description}</div>
            )}
            <Popover
                open={open}
                onOpenChange={(v) => {
                    setOpen(v)
                    if (v) setResults([]) // clear results each time dialog opens
                }}
            >
                <PopoverTrigger asChild>
                    <Button
                        variant="ghost"
                        className={
                            "ark-action-btn ark-delete absolute right-1 top-1/2 -translate-y-1/2 z-10 " +
                            (selected ? "!opacity-100 !pointer-events-auto" : "")
                        }
                        aria-label="Run tool"
                        title="Run"
                        tabIndex={-1}
                        onClick={(e) => {
                            e.stopPropagation()
                            setOpen(true)
                        }}
                    >
                        <Play className="h-4 w-4" />
                    </Button>
                </PopoverTrigger>
                <PopoverContent className="w-[420px]" onFocusOutside={(e) => e.preventDefault()}>
                    <div className="ark-form-title mb-2">{name}</div>
                    {renderSchemaForm({
                        schema: inputSchema,
                        values: formValues,
                        setValues: setFormValues,
                        onRun: async () => {
                            try {
                                const maybe = await (onRun?.(coerceArgs()) as any)
                                const pretty = prettifyIfJson(maybe)
                                if (pretty && pretty.length > 0) setResults((prev) => [...prev, pretty])
                            } catch (_) {
                                // ignore on failure; result appended only on success
                            }
                        },
                        results,
                        onCopyAll: async () => {
                            const text = results.join('\n\n')
                            try {
                                await navigator.clipboard.writeText(text)
                            } catch {
                                // fallback: no-op if clipboard unavailable
                            }
                        },
                        onClear: () => setResults([]),
                    })}
                </PopoverContent>
            </Popover>
        </li>
    )
}

function renderSchemaForm({ schema, values, setValues, onRun, results, onCopyAll, onClear }: { schema: any, values: Record<string, any>, setValues: (v: Record<string, any>) => void, onRun?: () => void, results: string[], onCopyAll: () => void, onClear: () => void }) {
    const hasSchema = !!schema && typeof schema === 'object'
    const { properties = {}, required = [] } = hasSchema ? schema : {}
    const keys = Object.keys(properties)
    const hasInputs = keys.length > 0
    return (
        <form className="space-y-3" onClick={(e) => e.stopPropagation()}>
            {!hasInputs && (
                <div className="text-xs text-muted-foreground">This tool takes no input.</div>
            )}
            {hasInputs && keys.map((k) => {
                const prop: any = (properties as any)[k] || {}
                const type = prop.type || 'string'
                const isRequired = Array.isArray(required) && (required as any).includes(k)
                const label = prop.title || k
                const placeholder = prop.description || undefined
                return (
                    <div key={k} className="flex flex-col gap-1">
                        <label className="ark-field-label">
                            {label}
                            {isRequired && <span aria-hidden className="text-destructive">*</span>}
                        </label>
                        {renderInputForType({
                            name: k,
                            type,
                            placeholder,
                            value: values[k],
                            onChange: (nv: any) => setValues({ ...values, [k]: nv })
                        })}
                    </div>
                )
            })}
            <div className="pt-1">
                <Button
                    type="button"
                    variant="outline"
                    className="ark-ghost"
                    size="sm"
                    onClick={(e) => {
                        e.stopPropagation()
                        onRun?.()
                    }}
                >
                    Run tool
                </Button>
            </div>
            {results.length > 0 && (
                <div className="mt-3 border-t pt-2">
                    <div className="flex items-center justify-between mb-1">
                        <div className="ark-form-title text-xs">Results</div>
                        <div className="flex items-center gap-2">
                            <Button type="button" variant="outline" size="sm" className="ark-ghost" onClick={(e) => { e.stopPropagation(); onCopyAll() }} aria-label="Copy results">
                                <Copy className="h-3.5 w-3.5 mr-1" />
                            </Button>
                            <Button type="button" variant="outline" size="sm" className="ark-ghost" onClick={(e) => { e.stopPropagation(); onClear() }} aria-label="Clear results">
                                <Trash2 className="h-3.5 w-3.5 mr-1" />
                            </Button>
                        </div>
                    </div>
                    <ul role="listbox" aria-label="Results" className="max-h-40 overflow-auto rounded-md border bg-muted/30 p-2 space-y-2">
                        {results.map((r, i) => (
                            <li key={i} role="option" className="text-xs whitespace-pre-wrap break-words p-2 rounded bg-background">
                                {r}
                            </li>
                        ))}
                    </ul>
                </div>
            )}
        </form>
    )
}

function renderInputForType({ name, type, placeholder, value, onChange }: { name: string, type: string, placeholder?: string, value: any, onChange: (v: any) => void }) {
    switch (type) {
        case 'integer':
        case 'number':
            return (
                <Input
                    name={name}
                    type="number"
                    placeholder={placeholder}
                    value={value ?? ''}
                    onChange={(e) => onChange(e.target.value)}
                />
            )
        case 'boolean':
            return (
                <Checkbox
                    checked={Boolean(value)}
                    onCheckedChange={(v) => onChange(!!v)}
                />
            )
        case 'string':
        default:
            return (
                <Input
                    name={name}
                    type="text"
                    placeholder={placeholder}
                    value={value ?? ''}
                    onChange={(e) => onChange(e.target.value)}
                />
            )
    }
}
