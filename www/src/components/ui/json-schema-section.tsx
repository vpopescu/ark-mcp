import React from "react"
import { useFormContext } from "react-hook-form"
import { FormControl, FormDescription, FormField, FormItem, FormLabel, FormMessage } from "@/components/ui/form"
import { Input } from "@/components/ui/input"
import { Checkbox } from "@/components/ui/checkbox"

type JsonSchema = {
  type?: string | string[]
  properties?: Record<string, any>
  required?: string[]
  enum?: string[]
  oneOf?: any[]
  $ref?: string
  description?: string
  [k: string]: any
}

function isBool(schema: any) {
  const t = schema?.type
  return t === "boolean" || (Array.isArray(t) && t.includes("boolean"))
}
function isString(schema: any) {
  const t = schema?.type
  return t === "string" || (Array.isArray(t) && t.includes("string"))
}
function isNumber(schema: any) {
  const t = schema?.type
  return t === "number" || t === "integer" || (Array.isArray(t) && (t.includes("number") || t.includes("integer")))
}
function allowsNull(schema: any) {
  const t = schema?.type
  return t === "null" || (Array.isArray(t) && t.includes("null"))
}

function includesType(t: any, needle: string) {
  return t === needle || (Array.isArray(t) && t.includes(needle))
}

function isObjectSchema(s: any) {
  if (!s) return false
  if (s.$ref) return true
  if (includesType(s.type, "object")) return true
  if (s.properties && typeof s.properties === "object") return true
  return false
}

function resolveRef(root: any, ref: string): any | undefined {
  if (!ref || typeof ref !== "string" || !ref.startsWith("#")) return undefined
  const path = ref.slice(2).split("/") // remove "#/"
  let cur: any = root
  for (const seg of path) {
    if (!cur) return undefined
    cur = cur[seg]
  }
  return cur
}

export function JsonSchemaSection({ schema, fieldPrefix, rootSchema }: { schema: JsonSchema; fieldPrefix?: string; rootSchema?: any }) {
  const { control, watch } = useFormContext()
  const props = schema?.properties || {}
  const required = new Set<string>(schema?.required || [])
  const keys = Object.keys(props)
  if (keys.length === 0) return null

  return (
    <div className="grid gap-4">
      {keys.map((key) => {
        const s = props[key] || {}
        const visible = s["x-ui-visible"] !== false
        if (!visible) return null
        const name = fieldPrefix ? `${fieldPrefix}.${key}` : key
        const displayName = typeof s["x-ui-display-name"] === "string" && s["x-ui-display-name"].trim().length > 0 ? (s["x-ui-display-name"] as string) : undefined
        const title = typeof s.title === "string" && s.title.trim().length > 0 ? (s.title as string) : undefined
        const fallback = key.replace(/[_-]+/g, " ").replace(/\b\w/g, (m) => m.toUpperCase())
        const label = displayName || title || fallback
        const isReq = required.has(key)
        const desc = typeof s.description === "string" ? (s.description as string) : ""
        const divider = s["x-ui-divider"]

        if (isObjectSchema(s)) {
          const nestedSchema = s.$ref && rootSchema ? resolveRef(rootSchema, s.$ref) : (s.properties ? s : undefined)
          // Special inline layout for management endpoints' path+enabled (livez/readyz)
          const isMgmtPath = (key === "livez" || key === "readyz") && nestedSchema && nestedSchema.properties?.path && nestedSchema.properties?.enabled

          if (isMgmtPath) {
            const pathSchema = nestedSchema.properties.path || {}
            const enabledSchema = nestedSchema.properties.enabled || {}
            const pathLabel = (pathSchema["x-ui-display-name"] as string) || "path"
            const enabledLabel = (enabledSchema["x-ui-display-name"] as string) || "enabled"
            const pathDesc = typeof pathSchema.description === "string" ? pathSchema.description : ""
            const enabledDesc = typeof enabledSchema.description === "string" ? enabledSchema.description : ""

            const patternStr = typeof pathSchema.pattern === "string" && pathSchema.pattern.length > 0 ? pathSchema.pattern : undefined
            const patternRe = patternStr ? new RegExp(patternStr) : undefined
            const pathRules: any = {}
            if (patternRe) {
              pathRules.validate = (val: any) => {
                if (val === undefined || val === null || val === "") return true
                return patternRe.test(String(val)) || `Must match pattern ${patternStr}`
              }
            }

            return (
              <div key={name} className="grid gap-2">
                {divider ? (
                  <div className="mt-2 border-t pt-4">
                    <div className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">{typeof divider === "string" ? divider : label}</div>
                  </div>
                ) : null}
                <div className="ark-form-name">
                  {label}
                  {isReq ? <span aria-hidden className="text-destructive">*</span> : null}
                </div>
                <div className="ark-form-description">{desc}</div>
        <div className="mt-2 flex flex-row items-end gap-4 flex-nowrap">
                  <FormField
                    control={control}
                    name={`${name}.path` as any}
                    rules={pathRules}
                    render={({ field }) => (
                      <FormItem className="flex-1 min-w-0">
                        <FormLabel className="ark-form-name">{pathLabel}</FormLabel>
                        <FormControl>
                          <Input type="text" pattern={patternStr} {...field} />
                        </FormControl>
                        <FormDescription>{pathDesc}</FormDescription>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                  <FormField
                    control={control}
                    name={`${name}.enabled` as any}
                    render={({ field }) => (
            <FormItem className="flex flex-row items-center gap-2 shrink-0">
                        <FormLabel className="m-0 ark-form-name">{enabledLabel}</FormLabel>
                        <FormControl>
              <Checkbox checked={!!field.value} onCheckedChange={(v) => field.onChange(!!v)} />
                        </FormControl>
                        <FormDescription>{enabledDesc}</FormDescription>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                </div>
              </div>
            )
          }

          return (
            <div key={name} className="grid gap-1">
              {divider ? (
                <div className="mt-2 border-t pt-4">
                  <div className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">{typeof divider === "string" ? divider : label}</div>
                </div>
              ) : null}
              <div className="ark-form-name">
                {label}
                {isReq ? <span aria-hidden className="text-destructive">*</span> : null}
              </div>
              <div className="ark-form-description">{desc}</div>
              {nestedSchema ? (
                <div className="ml-[4em] mt-2">
                  <JsonSchemaSection schema={nestedSchema} fieldPrefix={name} rootSchema={rootSchema || schema} />
                </div>
              ) : null}
            </div>
          )
        }

  const hasEnum = Array.isArray(s.enum)
  // Conditional disable: if this field is 'cors' and the parent object has 'add_cors_headers',
  // disable the input when add_cors_headers is unchecked.
  const isCorsField = key === "cors"
  const hasAddCorsSibling = isCorsField && !!props["add_cors_headers"]
  const addCorsPath = fieldPrefix ? `${fieldPrefix}.add_cors_headers` : "add_cors_headers"
  const corsDisabled = hasAddCorsSibling ? watch(addCorsPath) === false : false
        const nullable = allowsNull(s)

        // Build validation rules (pattern, etc.) for react-hook-form
        const patternStr = typeof s.pattern === "string" && s.pattern.length > 0 ? s.pattern : undefined
        const patternRe = patternStr ? new RegExp(patternStr) : undefined
        const rules: any = {}
        if (patternRe && isString(s)) {
          // Only enforce when value is present; allow empty when field allows null/omitted
          rules.validate = (val: any) => {
            if (val === undefined || val === null || val === "") return true
            return patternRe.test(String(val)) || `Must match pattern ${patternStr}`
          }
        }

        return (
          <FormField
            key={name}
            control={control}
            name={name as any}
            rules={rules}
            render={({ field }) => (
              <FormItem>
                {divider ? (
                  <div className="mt-2 border-t pt-4">
                    <div className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">{typeof divider === "string" ? divider : label}</div>
                  </div>
                ) : null}
                <FormLabel className="ark-form-name">
                  {label}
                  {isReq ? <span aria-hidden className="text-destructive">*</span> : null}
                </FormLabel>
                <FormControl>
                  {hasEnum ? (
                    <select
                      className="h-9 w-full rounded-md border border-input bg-background px-3 py-1 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
                      value={field.value ?? ""}
                      onChange={(e) => field.onChange(e.target.value)}
                    >
                      {nullable ? <option value="">(none)</option> : null}
                      {s.enum.map((opt: string) => (
                        <option key={opt} value={opt}>{opt}</option>
                      ))}
                    </select>
                  ) : isBool(s) ? (
                    <Checkbox checked={!!field.value} onCheckedChange={(v) => field.onChange(!!v)} />
                  ) : isNumber(s) ? (
                    <Input type="number" {...field} />
                  ) : (
                    <Input type="text" pattern={patternStr} disabled={corsDisabled} {...field} />
                  )}
                </FormControl>
                <FormDescription>{desc}</FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />
        )
      })}
    </div>
  )
}

export default JsonSchemaSection
