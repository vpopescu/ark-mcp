import React from "react"
import { useForm } from "react-hook-form"
import { z } from "zod"
import { zodResolver } from "@hookform/resolvers/zod"

import { Form, FormControl, FormDescription, FormField, FormItem, FormLabel, FormMessage } from "@/components/ui/form"
import { Input } from "@/components/ui/input"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Copy } from "lucide-react"
import schemaJson from "../../../ark.config.schema.json"
import { JsonSchemaSection } from "@/components/ui/json-schema-section"

const httpUrl = z.string()
    .url("Must be a valid URL")
    .regex(/^https?:\/\//i, "Must start with http:// or https://")

const SettingsSchema = z.object({
    apiBase: z.union([z.literal(""), httpUrl]).default(""),
    mcpBase: z.union([z.literal(""), httpUrl]).default(""),
    // Store a partial Ark config object shaped by the schema; we don’t validate fully here
    ark: z.any().default({})
})

type SettingsValues = z.infer<typeof SettingsSchema>

const STORAGE_KEY = "ark.ui.settings"

function loadInitial(): SettingsValues {
    try {
        const raw = localStorage.getItem(STORAGE_KEY)
        if (raw) {
            const parsed = JSON.parse(raw)
            const safe = SettingsSchema.partial().catch({}).parse(parsed)
            return {
                apiBase: safe.apiBase ?? "",
                mcpBase: safe.mcpBase ?? "",
                ark: safe.ark ?? defaultArkValues(),
            }
        }
    } catch { }
    return { apiBase: "", mcpBase: "" }
}

function defaultArkValues() {
    try {
        const s: any = schemaJson
        const props = s?.properties || {}
        const v: any = {}
        for (const k of Object.keys(props)) {
            const p = props[k]
            if (p && Object.prototype.hasOwnProperty.call(p, "default")) v[k] = p.default
        }
        return v
    } catch {
        return {}
    }
}

// Build defaults recursively from the schema, supporting simple $ref to $defs and object properties
function resolveRef(ref: string, root: any): any | undefined {
    if (typeof ref !== "string") return undefined
    if (ref.startsWith("#/$defs/")) {
        const key = ref.substring("#/$defs/".length)
        return root?.$defs?.[key]
    }
    return undefined
}

function schemaDefaults(schema: any, root: any): any {
    if (!schema) return undefined
    if (Object.prototype.hasOwnProperty.call(schema, "default")) {
        return schema.default
    }
    if (schema.$ref) {
        const target = resolveRef(schema.$ref, root)
        return schemaDefaults(target, root)
    }
    const hasProps = schema && typeof schema === "object" && schema.properties && typeof schema.properties === "object"
    const isObjType = schema?.type === "object" || hasProps
    if (isObjType) {
        const out: any = {}
        const props = schema.properties || {}
        for (const key of Object.keys(props)) {
            const d = schemaDefaults(props[key], root)
            if (d !== undefined) out[key] = d
        }
        return Object.keys(out).length ? out : undefined
    }
    // arrays/other: rely on explicit default only
    return undefined
}

function deepEqual(a: any, b: any): boolean {
    if (a === b) return true
    if (a == null || b == null) return false
    if (Array.isArray(a) && Array.isArray(b)) {
        if (a.length !== b.length) return false
        for (let i = 0; i < a.length; i++) if (!deepEqual(a[i], b[i])) return false
        return true
    }
    if (typeof a === "object" && typeof b === "object") {
        const ak = Object.keys(a)
        const bk = Object.keys(b)
        if (ak.length !== bk.length) return false
        for (const k of ak) if (!deepEqual(a[k], (b as any)[k])) return false
        return true
    }
    return false
}

function pruneWithDefaults(value: any, defaults: any): any | undefined {
    if (value === null || value === undefined) return undefined
    // If no defaults, still prune nulls and empty children for objects
    if (Array.isArray(value)) {
        if (defaults !== undefined && Array.isArray(defaults) && deepEqual(value, defaults)) return undefined
        return value
    }
    if (typeof value === "object") {
        const out: any = {}
        const keys = Object.keys(value)
        for (const k of keys) {
            const pruned = pruneWithDefaults(value[k], defaults ? defaults[k] : undefined)
            if (pruned !== undefined) out[k] = pruned
        }
        return Object.keys(out).length ? out : undefined
    }
    // primitive
    if (defaults !== undefined && deepEqual(value, defaults)) return undefined
    return value
}

function computeArkDefaultsFromSchema(): any {
    try {
        const root: any = schemaJson
        const props = root?.properties || {}
        const out: any = {}
        for (const k of Object.keys(props)) {
            const d = schemaDefaults(props[k], root)
            if (d !== undefined) out[k] = d
        }
        return out
    } catch {
        return {}
    }
}

export function SettingsForm() {
    const form = useForm<SettingsValues>({
        resolver: zodResolver(SettingsSchema),
        defaultValues: loadInitial(),
        mode: "onChange",
    })
    const [showPreview, setShowPreview] = React.useState(false)

    // Prepare labels, enums, and descriptions for specific Ark fields from the schema
    const schemaProps: any = (schemaJson as any)?.properties || {}
    const logLevelSchema: any = schemaProps.log_level || {}
    const transportSchema: any = schemaProps.transport || {}
    const managementProp: any = schemaProps.management_server || {}
    const mcpProp: any = schemaProps.mcp_server || {}
    const logLevelLabel: string = (typeof logLevelSchema["x-ui-display-name"] === "string" && logLevelSchema["x-ui-display-name"]) || (logLevelSchema.title as string) || "Log Level"
    const transportLabel: string = (typeof transportSchema["x-ui-display-name"] === "string" && transportSchema["x-ui-display-name"]) || (transportSchema.title as string) || "Transport"
    const logLevelDesc: string = typeof logLevelSchema.description === "string" ? logLevelSchema.description : ""
    const transportDesc: string = typeof transportSchema.description === "string" ? transportSchema.description : ""
    const managementLabel: string = (typeof managementProp["x-ui-display-name"] === "string" && managementProp["x-ui-display-name"]) || (managementProp.title as string) || "Management endpoint configuration"
    const mcpLabel: string = (typeof mcpProp["x-ui-display-name"] === "string" && mcpProp["x-ui-display-name"]) || (mcpProp.title as string) || "MCP endpoint configuration"
    const managementDesc: string = typeof managementProp.description === "string" ? managementProp.description : ""
    const mcpDesc: string = typeof mcpProp.description === "string" ? mcpProp.description : ""
    const managementSchema: any = (schemaJson as any)?.$defs?.ManagementEndpointConfig
    const mcpSchema: any = (schemaJson as any)?.$defs?.McpEndpointConfig
    const mgmtAddCors = managementSchema?.properties?.add_cors_headers || {}
    const mgmtCors = managementSchema?.properties?.cors || {}
    const mcpAddCors = mcpSchema?.properties?.add_cors_headers || {}
    const mcpCors = mcpSchema?.properties?.cors || {}
    const mgmtBind = managementSchema?.properties?.bind_address || {}
    const mcpBind = mcpSchema?.properties?.bind_address || {}
    const mgmtLivez = managementSchema?.properties?.livez || {}
    const mgmtReadyz = managementSchema?.properties?.readyz || {}
    const pathDef = (schemaJson as any)?.$defs?.ManagementPathConfig?.properties?.path || {}
    const enabledDef = (schemaJson as any)?.$defs?.ManagementPathConfig?.properties?.enabled || {}
    const mgmtAddCorsLabel: string = (mgmtAddCors["x-ui-display-name"] as string) || mgmtAddCors.title || "add_cors_headers"
    const mgmtCorsLabel: string = (mgmtCors["x-ui-display-name"] as string) || mgmtCors.title || "cors"
    const mcpAddCorsLabel: string = (mcpAddCors["x-ui-display-name"] as string) || mcpAddCors.title || "add_cors_headers"
    const mcpCorsLabel: string = (mcpCors["x-ui-display-name"] as string) || mcpCors.title || "cors"
    const mgmtBindLabel: string = (mgmtBind["x-ui-display-name"] as string) || mgmtBind.title || "bind_address"
    const mcpBindLabel: string = (mcpBind["x-ui-display-name"] as string) || mcpBind.title || "bind_address"
    const mgmtLivezLabel: string = (mgmtLivez["x-ui-display-name"] as string) || mgmtLivez.title || "livez"
    const mgmtReadyzLabel: string = (mgmtReadyz["x-ui-display-name"] as string) || mgmtReadyz.title || "readyz"
    const pathLabel: string = (pathDef["x-ui-display-name"] as string) || pathDef.title || "path"
    const mgmtAddCorsDesc: string = typeof mgmtAddCors.description === "string" ? mgmtAddCors.description : ""
    const mgmtCorsDesc: string = typeof mgmtCors.description === "string" ? mgmtCors.description : ""
    const mcpAddCorsDesc: string = typeof mcpAddCors.description === "string" ? mcpAddCors.description : ""
    const mcpCorsDesc: string = typeof mcpCors.description === "string" ? mcpCors.description : ""
    const mgmtBindDesc: string = typeof mgmtBind.description === "string" ? mgmtBind.description : ""
    const mcpBindDesc: string = typeof mcpBind.description === "string" ? mcpBind.description : ""
    const mgmtLivezDesc: string = typeof mgmtLivez.description === "string" ? mgmtLivez.description : ""
    const mgmtReadyzDesc: string = typeof mgmtReadyz.description === "string" ? mgmtReadyz.description : ""
    const pathDesc: string = typeof pathDef.description === "string" ? pathDef.description : ""

    const managementFiltered = React.useMemo(() => {
        if (!managementSchema) return undefined as any
        const props = { ...(managementSchema.properties || {}) }
        delete props.add_cors_headers
        delete props.cors
    delete props.bind_address
    delete props.disable_api
    delete props.disable_console
    delete props.livez
    delete props.readyz
        return { ...managementSchema, properties: props }
    }, [managementSchema])
    const mcpFiltered = React.useMemo(() => {
        if (!mcpSchema) return undefined as any
        const props = { ...(mcpSchema.properties || {}) }
        delete props.add_cors_headers
        delete props.cors
    delete props.bind_address
        return { ...mcpSchema, properties: props }
    }, [mcpSchema])
    const mgmtAddCorsChecked = form.watch("ark.management_server.add_cors_headers" as any)
    const mcpAddCorsChecked = form.watch("ark.mcp_server.add_cors_headers" as any)
    const mgmtLivezEnabled = form.watch("ark.management_server.livez.enabled" as any)
    const mgmtReadyzEnabled = form.watch("ark.management_server.readyz.enabled" as any)
    const logLevelEnum: string[] = Array.isArray(logLevelSchema.enum) ? logLevelSchema.enum : []
    const transportEnum: string[] = Array.isArray(transportSchema.enum) ? transportSchema.enum : []
    const logLevelAllowsNull = (Array.isArray(logLevelSchema.type) && logLevelSchema.type.includes("null")) || logLevelSchema.type === "null"
    const transportAllowsNull = (Array.isArray(transportSchema.type) && transportSchema.type.includes("null")) || transportSchema.type === "null"

    // Filter out fields that we render in a dedicated card to avoid duplication in the schema-driven section
    const filteredSchema = React.useMemo(() => {
        const s: any = schemaJson as any
        const props = { ...(s?.properties || {}) }
        delete props.log_level
        delete props.transport
    delete props.management_server
    delete props.mcp_server
        return { ...s, properties: props }
    }, [])

    function onSubmit(values: SettingsValues) {
        try {
            localStorage.setItem(STORAGE_KEY, JSON.stringify(values))
        } catch { }
    }

    return (
        <div className="rounded-md border bg-card p-6 text-card-foreground">
            <h2 className="text-lg font-semibold mb-1">Settings</h2>
            <p className="text-sm text-muted-foreground mb-4">Ark server configuration. Please note that changing settings will require that the server is restarted manually at this time.</p>
            <Form {...form}>
                <form onSubmit={form.handleSubmit(onSubmit)} className="grid gap-6 max-w-3xl">
                                <Card>
                                    <CardHeader>
                                        <CardTitle className="ark-settings-card-title">Admin console server base URLs</CardTitle>
                                        <CardDescription className="ark-settings-card-description">Set custom base URLs for the admin console to reach Ark server endpoints.</CardDescription>
                                    </CardHeader>
                                    <CardContent>
                                        <div className="grid gap-4 md:grid-cols-2 items-start">
                                            <FormField
                                                control={form.control}
                                                name="apiBase"
                                                render={({ field }) => (
                                                    <FormItem>
                                                        <FormLabel className="ark-form-name">API Base URL</FormLabel>
                                                        <FormControl>
                                                            <Input placeholder="http://localhost:8000" {...field} />
                                                        </FormControl>
                                                        <FormDescription>Overrides API base used for /api calls.</FormDescription>
                                                        <FormMessage />
                                                    </FormItem>
                                                )}
                                            />

                                            <FormField
                                                control={form.control}
                                                name="mcpBase"
                                                render={({ field }) => (
                                                    <FormItem>
                                                        <FormLabel className="ark-form-name">MCP Base URL</FormLabel>
                                                        <FormControl>
                                                            <Input placeholder="http://localhost:3001" {...field} />
                                                        </FormControl>
                                                        <FormDescription>Overrides MCP base used for tool execution.</FormDescription>
                                                        <FormMessage />
                                                    </FormItem>
                                                )}
                                            />
                                        </div>
                                    </CardContent>
                                </Card>

                                <Card>
                                    <CardHeader>
                                        <CardTitle className="ark-settings-card-title">ARK server configuration</CardTitle>
                                        <CardDescription className="ark-settings-card-description">Set core runtime options for the Ark server.</CardDescription>
                                    </CardHeader>
                                    <CardContent>
                                        <div className="grid gap-4 md:grid-cols-2 items-start">
                                            <FormField
                                                control={form.control}
                                                name="ark.log_level"
                                                render={({ field }) => (
                                                    <FormItem>
                                                        <FormLabel className="ark-form-name">{logLevelLabel}</FormLabel>
                                                        <FormControl>
                                                            <select
                                                                className="h-9 w-full rounded-md border border-input bg-background px-3 py-1 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
                                                                value={field.value ?? ""}
                                                                onChange={(e) => field.onChange(e.target.value)}
                                                            >
                                                                {logLevelAllowsNull ? <option value="">(none)</option> : null}
                                                                {logLevelEnum.map((opt) => (
                                                                    <option key={opt} value={opt}>{opt}</option>
                                                                ))}
                                                            </select>
                                                        </FormControl>
                                                        <FormDescription>{logLevelDesc}</FormDescription>
                                                        <FormMessage />
                                                    </FormItem>
                                                )}
                                            />

                                            <FormField
                                                control={form.control}
                                                name="ark.transport"
                                                render={({ field }) => (
                                                    <FormItem>
                                                        <FormLabel className="ark-form-name">{transportLabel}</FormLabel>
                                                        <FormControl>
                                                            <select
                                                                className="h-9 w-full rounded-md border border-input bg-background px-3 py-1 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
                                                                value={field.value ?? ""}
                                                                onChange={(e) => field.onChange(e.target.value)}
                                                            >
                                                                {transportAllowsNull ? <option value="">(none)</option> : null}
                                                                {transportEnum.map((opt) => (
                                                                    <option key={opt} value={opt}>{opt}</option>
                                                                ))}
                                                            </select>
                                                        </FormControl>
                                                        <FormDescription>{transportDesc}</FormDescription>
                                                        <FormMessage />
                                                    </FormItem>
                                                )}
                                            />
                                        </div>
                                    </CardContent>
                                </Card>

                                <Card>
                                    <CardHeader>
                                        <CardTitle className="ark-settings-card-title">ARK server endpoint configuration</CardTitle>
                                        <CardDescription className="ark-settings-card-description">Configure network endpoints for the Ark management and MCP servers.</CardDescription>
                                    </CardHeader>
                                    <CardContent>
                                        <div className="grid gap-6 items-start md:grid-cols-[1fr_1px_1fr]">
                                            <div className="md:pr-6">
                                                <CardTitle className="ark-settings-card-title ark-fakecard-title-padding">{managementLabel}</CardTitle>
                                                {managementDesc ? (
                                                    <CardDescription className="ark-settings-card-description">{managementDesc}</CardDescription>
                                                ) : null}
                                                <div className="mt-3">
                                                    <Card>
                                                        <CardHeader>
                                                            <CardTitle className="ark-settings-card-title">
                                                                <FormField
                                                                    control={form.control}
                                                                    name={"ark.management_server.add_cors_headers" as any}
                                                                    render={({ field }) => (
                                                                        <div className="flex items-center gap-2">
                                                                            <Checkbox
                                                                                checked={!!field.value}
                                                                                onCheckedChange={(v) => field.onChange(!!v)}
                                                                            />
                                                                            <span className="ark-form-name">{mgmtAddCorsLabel}</span>
                                                                        </div>
                                                                    )}
                                                                />
                                                            </CardTitle>
                                                        </CardHeader>
                                                        <CardContent className="pt-0">
                                                            <div className="ml-6 mt-2">
                                                                <FormField
                                                                    control={form.control}
                                                                    name={"ark.management_server.cors" as any}
                                                                    render={({ field }) => (
                                                                        <FormItem>
                                                                            <FormLabel className={`ark-form-name${!mgmtAddCorsChecked ? " text-muted-foreground" : ""}`}>{mgmtCorsLabel}</FormLabel>
                                                                            <FormControl>
                                                                                <Input type="text" disabled={!mgmtAddCorsChecked} {...field} />
                                                                            </FormControl>
                                                                            <FormDescription>{mgmtCorsDesc}</FormDescription>
                                                                            <FormMessage />
                                                                        </FormItem>
                                                                    )}
                                                                />
                                                            </div>
                                                        </CardContent>
                                                    </Card>
                                                </div>
                                                    <div className="mt-3">
                                                        <Card>
                                                            <CardHeader>
                                                                <CardTitle className="ark-settings-card-title">API controls</CardTitle>
                                                                <CardDescription className="ark-settings-card-description">Enable or disable API and Console endpoints.</CardDescription>
                                                            </CardHeader>
                                                            <CardContent>
                                                                <div className="grid gap-3">
                                                                    <FormField
                                                                        control={form.control}
                                                                        name={"ark.management_server.disable_api" as any}
                                                                        render={({ field }) => (
                                                                            <FormItem className="flex flex-row items-center gap-2">
                                                                                <FormControl>
                                                                                    <Checkbox
                                                                                        checked={!!field.value}
                                                                                        onCheckedChange={(v) => field.onChange(!!v)}
                                                                                    />
                                                                                </FormControl>
                                                                                <FormLabel className="m-0 ark-form-name">{(managementSchema?.properties?.disable_api?.["x-ui-display-name"] as string) || "disable_api"}</FormLabel>
                                                                                <FormDescription>{typeof managementSchema?.properties?.disable_api?.description === "string" ? managementSchema.properties.disable_api.description : ""}</FormDescription>
                                                                                <FormMessage />
                                                                            </FormItem>
                                                                        )}
                                                                    />
                                                                    <FormField
                                                                        control={form.control}
                                                                        name={"ark.management_server.disable_console" as any}
                                                                        render={({ field }) => (
                                                                            <FormItem className="flex flex-row items-center gap-2">
                                                                                <FormControl>
                                                                                    <Checkbox
                                                                                        checked={!!field.value}
                                                                                        onCheckedChange={(v) => field.onChange(!!v)}
                                                                                    />
                                                                                </FormControl>
                                                                                <FormLabel className="m-0 ark-form-name">{(managementSchema?.properties?.disable_console?.["x-ui-display-name"] as string) || "disable_console"}</FormLabel>
                                                                                <FormDescription>{typeof managementSchema?.properties?.disable_console?.description === "string" ? managementSchema.properties.disable_console.description : ""}</FormDescription>
                                                                                <FormMessage />
                                                                            </FormItem>
                                                                        )}
                                                                    />
                                                                </div>
                                                            </CardContent>
                                                        </Card>
                                                    </div>
                                                    <div className="mt-3">
                                                        <Card>
                                                            <CardHeader>
                                                                <CardTitle className="ark-settings-card-title">
                                                                    <FormField
                                                                        control={form.control}
                                                                        name={"ark.management_server.livez.enabled" as any}
                                                                        render={({ field }) => (
                                                                            <div className="flex items-center gap-2">
                                                                                <Checkbox
                                                                                    checked={!!field.value}
                                                                                    onCheckedChange={(v) => field.onChange(!!v)}
                                                                                />
                                                                                <span className="ark-form-name">{mgmtLivezLabel}</span>
                                                                            </div>
                                                                        )}
                                                                    />
                                                                </CardTitle>
                                                                {mgmtLivezDesc ? <CardDescription>{mgmtLivezDesc}</CardDescription> : null}
                                                            </CardHeader>
                                                            <CardContent className="pt-0">
                                                                <div className="ml-6 mt-2">
                                                                    <FormField
                                                                        control={form.control}
                                                                        name={"ark.management_server.livez.path" as any}
                                                                        render={({ field }) => (
                                                                            <FormItem>
                                                                                <FormLabel className={`ark-form-name${!mgmtLivezEnabled ? " text-muted-foreground" : ""}`}>{pathLabel}</FormLabel>
                                                                                <FormControl>
                                                                                    <Input type="text" disabled={!mgmtLivezEnabled} {...field} />
                                                                                </FormControl>
                                                                                <FormDescription>{pathDesc}</FormDescription>
                                                                                <FormMessage />
                                                                            </FormItem>
                                                                        )}
                                                                    />
                                                                </div>
                                                            </CardContent>
                                                        </Card>
                                                    </div>
                                                    <div className="mt-3">
                                                        <Card>
                                                            <CardHeader>
                                                                <CardTitle className="ark-settings-card-title">
                                                                    <FormField
                                                                        control={form.control}
                                                                        name={"ark.management_server.readyz.enabled" as any}
                                                                        render={({ field }) => (
                                                                            <div className="flex items-center gap-2">
                                                                                <Checkbox
                                                                                    checked={!!field.value}
                                                                                    onCheckedChange={(v) => field.onChange(!!v)}
                                                                                />
                                                                                <span className="ark-form-name">{mgmtReadyzLabel}</span>
                                                                            </div>
                                                                        )}
                                                                    />
                                                                </CardTitle>
                                                                {mgmtReadyzDesc ? <CardDescription>{mgmtReadyzDesc}</CardDescription> : null}
                                                            </CardHeader>
                                                            <CardContent className="pt-0">
                                                                <div className="ml-6 mt-2">
                                                                    <FormField
                                                                        control={form.control}
                                                                        name={"ark.management_server.readyz.path" as any}
                                                                        render={({ field }) => (
                                                                            <FormItem>
                                                                                <FormLabel className={`ark-form-name${!mgmtReadyzEnabled ? " text-muted-foreground" : ""}`}>{pathLabel}</FormLabel>
                                                                                <FormControl>
                                                                                    <Input type="text" disabled={!mgmtReadyzEnabled} {...field} />
                                                                                </FormControl>
                                                                                <FormDescription>{pathDesc}</FormDescription>
                                                                                <FormMessage />
                                                                            </FormItem>
                                                                        )}
                                                                    />
                                                                </div>
                                                            </CardContent>
                                                        </Card>
                                                    </div>
                                                    <div className="mt-3">
                                                        <Card>
                                                            <CardHeader>
                                                                <CardTitle className="ark-settings-card-title">Bind address</CardTitle>
                                                            </CardHeader>
                                                            <CardContent className="pt-0">
                                                                <FormField
                                                                    control={form.control}
                                                                    name={"ark.management_server.bind_address" as any}
                                                                    render={({ field }) => (
                                                                        <FormItem>
                                                                            <FormLabel className="ark-form-name">{mgmtBindLabel}</FormLabel>
                                                                            <FormControl>
                                                                                <Input type="text" {...field} />
                                                                            </FormControl>
                                                                            <FormDescription>{mgmtBindDesc}</FormDescription>
                                                                            <FormMessage />
                                                                        </FormItem>
                                                                    )}
                                                                />
                                                            </CardContent>
                                                        </Card>
                                                    </div>
                                                    <div className="mt-4">
                                                        <JsonSchemaSection schema={managementFiltered as any} fieldPrefix="ark.management_server" rootSchema={schemaJson as any} />
                                                    </div>
                                            </div>
                                            <div className="hidden md:block w-px self-stretch bg-[var(--border)]" aria-hidden />
                                            <div className="md:pl-6">
                                                <CardTitle className="ark-settings-card-title ark-fakecard-title-padding">{mcpLabel}</CardTitle>
                                                {mcpDesc ? (
                                                    <CardDescription className="ark-settings-card-description">{mcpDesc}</CardDescription>
                                                ) : null}
                                                <div className="mt-3">
                                                    <Card>
                                                        <CardHeader>
                                                            <CardTitle className="ark-settings-card-title">
                                                                <FormField
                                                                    control={form.control}
                                                                    name={"ark.mcp_server.add_cors_headers" as any}
                                                                    render={({ field }) => (
                                                                        <div className="flex items-center gap-2">
                                                                            <Checkbox
                                                                                checked={!!field.value}
                                                                                onCheckedChange={(v) => field.onChange(!!v)}
                                                                            />
                                                                            <span className="ark-form-name">{mcpAddCorsLabel}</span>
                                                                        </div>
                                                                    )}
                                                                />
                                                            </CardTitle>
                                                        </CardHeader>
                                                        <CardContent className="pt-0">
                                                            <div className="ml-6 mt-2">
                                                                <FormField
                                                                    control={form.control}
                                                                    name={"ark.mcp_server.cors" as any}
                                                                    render={({ field }) => (
                                                                        <FormItem>
                                                                            <FormLabel className={`ark-form-name${!mcpAddCorsChecked ? " text-muted-foreground" : ""}`}>{mcpCorsLabel}</FormLabel>
                                                                            <FormControl>
                                                                                <Input type="text" disabled={!mcpAddCorsChecked} {...field} />
                                                                            </FormControl>
                                                                            <FormDescription>{mcpCorsDesc}</FormDescription>
                                                                            <FormMessage />
                                                                        </FormItem>
                                                                    )}
                                                                />
                                                            </div>
                                                        </CardContent>
                                                    </Card>
                                                </div>
                                                    <div className="mt-3">
                                                        <Card>
                                                            <CardHeader>
                                                                <CardTitle className="ark-settings-card-title">Bind address</CardTitle>
                                                            </CardHeader>
                                                            <CardContent className="pt-0">
                                                                <FormField
                                                                    control={form.control}
                                                                    name={"ark.mcp_server.bind_address" as any}
                                                                    render={({ field }) => (
                                                                        <FormItem>
                                                                            <FormLabel className="ark-form-name">{mcpBindLabel}</FormLabel>
                                                                            <FormControl>
                                                                                <Input type="text" {...field} />
                                                                            </FormControl>
                                                                            <FormDescription>{mcpBindDesc}</FormDescription>
                                                                            <FormMessage />
                                                                        </FormItem>
                                                                    )}
                                                                />
                                                            </CardContent>
                                                        </Card>
                                                    </div>
                                                <div className="mt-4">
                                                    <JsonSchemaSection schema={mcpFiltered as any} fieldPrefix="ark.mcp_server" rootSchema={schemaJson as any} />
                                                </div>
                                            </div>
                                        </div>
                                    </CardContent>
                                </Card>


                    <div>
                        
                        <JsonSchemaSection schema={filteredSchema as any} fieldPrefix="ark" rootSchema={schemaJson as any} />
                    </div>

                    <div className="flex gap-2">
                        <Button type="submit" disabled>Save</Button>
                        <Button
                            type="button"
                            variant="secondary"
                            onClick={() => {
                                form.reset({ apiBase: "", mcpBase: "", ark: defaultArkValues() })
                                try { localStorage.removeItem(STORAGE_KEY) } catch { }
                            }}
                        >
                            Reset
                        </Button>
                        <Button
                            type="button"
                            variant="ghost"
                            onClick={() => setShowPreview(true)}
                        >
                            Preview
                        </Button>
                    </div>
                    <Dialog open={showPreview} onOpenChange={setShowPreview}>
                        <DialogContent>
                            <DialogHeader>
                                <DialogTitle>Preview config.json</DialogTitle>
                                <DialogDescription>Read-only view. Right-click to copy, or use the Copy button.</DialogDescription>
                            </DialogHeader>
                            {(() => {
                                const v = form.getValues()
                                const current = (v?.ark ?? {}) as any
                                const defaults = computeArkDefaultsFromSchema()
                                const pruned = pruneWithDefaults(current, defaults) ?? {}
                                const text = JSON.stringify(pruned, null, 2)
                                return (
                                    <div className="relative">
                                        <Button
                                            type="button"
                                            className="absolute right-0 -top-10 h-8 px-2"
                                            variant="secondary"
                                            onClick={async () => {
                                                try {
                                                    await navigator.clipboard.writeText(text)
                                                } catch {
                                                    // Fallback: attempt execCommand on a temporary textarea
                                                    try {
                                                        const ta = document.createElement("textarea")
                                                        ta.value = text
                                                        ta.style.position = "fixed"
                                                        ta.style.opacity = "0"
                                                        document.body.appendChild(ta)
                                                        ta.select()
                                                        document.execCommand("copy")
                                                        document.body.removeChild(ta)
                                                    } catch { /* no-op */ }
                                                }
                                            }}
                                            title="Copy"
                                        >
                                            <Copy className="size-4" />
                                        </Button>
                                        <textarea
                                            readOnly
                                            value={text}
                                            className="w-full h-[60vh] font-mono text-xs bg-muted p-3 rounded border outline-none"
                                        />
                                    </div>
                                )
                            })()}
                        </DialogContent>
                    </Dialog>
                </form>
            </Form>
        </div>
    )
}

export default SettingsForm
