import { Input } from "@/components/ui/input"
import { useState } from "react"
import { Filter, Plus, Upload } from "lucide-react"
import {
    Popover,
    PopoverContent,
    PopoverTrigger,
} from "@/components/ui/popover"
import { Button } from "@/components/ui/button"
// Switched to Popover from HoverCard for click-open behavior
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import axios from "axios"
import { getApiBase } from "@/lib/config"
import { AuthState } from "@/lib/auth"



export function pluginListHeader({ onError, onRefresh, auth }: { onError?: (msg: string) => void, onRefresh?: () => void, auth?: AuthState }) {
    const [urlText, setUrlText] = useState("")
    const [submitting, setSubmitting] = useState(false)

    async function addPluginFromUrl() {
        const raw = (urlText || "").trim()
        if (!raw) return
        try {
            setSubmitting(true)
            // Derive a name from the URL/path
            let name = "plugin"
            try {
                // Works for http/https/file
                const u = new URL(raw)
                const last = u.pathname.split("/").filter(Boolean).pop() || u.hostname || "plugin"
                name = last.replace(/\.[^/.]+$/, "") || "plugin"
            } catch {
                // Non-standard like oci: or bare paths
                const seg = raw.split(/[\/]/).filter(Boolean).pop() || raw
                name = (seg.split("/").pop() || seg).replace(/\.[^/.]+$/, "") || "plugin"
            }
            const apiBase = getApiBase()
            const postUrl = `${apiBase}/api/plugins`
            await axios.post(postUrl, { name, url: raw })
            if (import.meta.env.DEV) console.debug("Plugin POSTed from URL:", { name, url: raw })
            setUrlText("")
            onRefresh?.()
        } catch (err: any) {
            const apiBase = getApiBase()
            const postUrl = `${apiBase}/api/plugins`
            const msg = `${typeof err?.message === 'string' ? err.message : 'Failed to add plugin from URL.'} (url: ${postUrl})`
            onError?.(msg)
        } finally {
            setSubmitting(false)
        }
    }
    async function browseLocalWasmAndPost() {
        try {
            let file: File | null = null

            // Preferred: File System Access API
            // @ts-expect-error - showOpenFilePicker is not in lib.dom yet for all targets
            if (window.showOpenFilePicker) {
                // @ts-expect-error see above
                const handles: FileSystemFileHandle[] = await window.showOpenFilePicker({
                    multiple: false,
                    excludeAcceptAllOption: true,
                    types: [
                        {
                            description: "WebAssembly module",
                            accept: { "application/wasm": [".wasm"] },
                        },
                    ],
                })
                if (handles && handles[0]) {
                    file = await handles[0].getFile()
                }
            }

            // Fallback: classic input[type=file]
            if (!file) {
                file = await new Promise<File | null>((resolve) => {
                    const input = document.createElement("input")
                    input.type = "file"
                    input.accept = ".wasm"
                    input.multiple = false
                    input.onchange = () => {
                        const f = input.files && input.files[0] ? input.files[0] : null
                        resolve(f)
                        // cleanup
                        input.remove()
                    }
                    input.click()
                })
            }

            if (!file) return

            const fileName = file.name
            const baseName = fileName.replace(/\.[^/.]+$/, "")

            // Attempt to get absolute path (available only in certain environments like Electron/WebView)
            const anyFile: any = file as any
            const rawPath: string | undefined = anyFile.path || anyFile.webkitRelativePath

            if (!rawPath) {
                // Browser cannot provide absolute path securely; route to app-level error banner
                onError?.(
                    "This browser cannot provide the absolute file path. To add a local plugin by path, please use an environment that can expose file paths (e.g., Electron/Tauri) or switch to the URL tab and paste the full path."
                )
                return
            }

            // Normalize Windows backslashes just in case backend expects native path
            const absolutePath = rawPath

            const apiBase = getApiBase()
            const url = `${apiBase}/api/plugins`
            await axios.post(url, {
                name: baseName,
                url: absolutePath,
            })

            // Optional: simple success hint
            if (import.meta.env.DEV) console.debug("Plugin POSTed:", { name: baseName, url: absolutePath })
            onRefresh?.()
        } catch (err: any) {
            console.error("Failed to add plugin from local file:", err)
            const apiBase = getApiBase()
            const url = `${apiBase}/api/plugins`
            const msg = `${typeof err?.message === 'string' ? err.message : 'Failed to add plugin.'} (url: ${url})`
            onError?.(msg)
        }
    }
    return (
        <div className="ark-plugins-list flex items-center gap-3">
            <div className="ark-section-title">Plugins list</div>

            <div className="ml-auto ark-section-filter">

                <Popover>
                    <PopoverTrigger asChild>
                        <Button variant="outline" className="ark-ghost cursor-pointer" disabled={!auth?.authenticated}>
                            <Plus className=" h-4 w-4" />
                        </Button>
                    </PopoverTrigger>
                    <PopoverContent className="w-[400px]" onFocusOutside={(e) => e.preventDefault()}>

                        <div className="ark-form-title">Add a plugin</div>

                        <Tabs defaultValue="URL" >
                            <TabsList className="w-full justify-start rounded-none border-b bg-transparent p-0" >
                                <TabsTrigger value="URL">URL</TabsTrigger>
                                <TabsTrigger value="LocalFile" disabled title="Local file uploads are disabled in this build">Local file</TabsTrigger>
                            </TabsList>

                            <TabsContent value="URL">
                                <div className="ark-field-label">URL (https, http, file, or oci):</div>


                                <div className="flex items-center gap-2">

                                    <Input
                                        type="text"
                                        placeholder="URL"
                                        aria-label="Plugin URL"
                                        value={urlText}
                                        onChange={(e) => setUrlText(e.target.value)}
                                        onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); addPluginFromUrl() } }}
                                        className=" rounded-md  text-sm placeholder:text-muted-foreground "
                                    />
                                    <Button variant="outline" className="ark-ghost" onClick={addPluginFromUrl} disabled={!urlText.trim() || submitting}>
                                        <Upload />
                                    </Button>
                                </div>
                            </TabsContent>
                            <TabsContent value="LocalFile">
                                <div className="text-xs text-muted-foreground">Local file uploads are disabled.</div>
                            </TabsContent>
                        </Tabs >
                    </PopoverContent>
                </Popover>
                <Popover>
                    <PopoverTrigger asChild>
                        <Button variant="outline" className="ark-ghost" hidden={true}>
                            <Filter className=" h-4 w-4" />
                        </Button>
                    </PopoverTrigger>
                    <PopoverContent className="w-[400px]" onFocusOutside={(e) => e.preventDefault()}>
                        <div className="ark-field-label">Filter plugins by name</div>
                        <Input
                            type="text"
                            placeholder="Filter plugins"
                            aria-label="Filter plugins"
                            className="h-9 w-full rounded-md  bg-background px-3 text-sm placeholder:text-muted-foreground "
                        />
                    </PopoverContent>
                </Popover>

            </div>


        </div>

    )
}
