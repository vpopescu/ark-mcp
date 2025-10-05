import path from "path"
import tailwindcss from "@tailwindcss/vite"
import react from "@vitejs/plugin-react"
import { defineConfig, loadEnv } from "vite"

// https://vite.dev/config/
export default defineConfig(({ mode }) => {

  const env = loadEnv(mode, process.cwd(), "")
  const api = process.env.VITE_ARK_SERVER_API || process.env.ARK_SERVER_API || env.VITE_ARK_SERVER_API || env.ARK_SERVER_API || ""
  const mcp = process.env.VITE_ARK_SERVER_MCP || process.env.ARK_SERVER_MCP || env.VITE_ARK_SERVER_MCP || env.ARK_SERVER_MCP || ""
  return {
    base: "/admin/",
    plugins: [react(), tailwindcss()],
    resolve: {
      alias: {
        "@": path.resolve(__dirname, "./src"),
      },
      // Ensure a single React instance to avoid invalid hook calls or
      // "useLayoutEffect" errors when packages bundle their own React.
      dedupe: ["react", "react-dom", "react/jsx-runtime"],
    },
    optimizeDeps: {
      include: ["react", "react-dom", "react/jsx-runtime"],
    },
    define: {
      // Surface server endpoints to client even if env vars lack VITE_ prefix
      "import.meta.env.VITE_ARK_SERVER_API": JSON.stringify(api),
      "import.meta.env.VITE_ARK_SERVER_MCP": JSON.stringify(mcp),
      // Also expose non-VITE keys for optional fallback reads
      "import.meta.env.ARK_SERVER_API": JSON.stringify(api),
      "import.meta.env.ARK_SERVER_MCP": JSON.stringify(mcp),
    },
    // Split big vendor bundles into smaller chunks and relax warning limit
    build: {
      chunkSizeWarningLimit: 900, // raise from 500kB to 900kB
      rollupOptions: {
        output: {
          manualChunks: {
            // Split vendor libraries into separate chunks
            vendor: ['react', 'react-dom'],
            ui: ['lucide-react'],
            utils: ['axios', 'ajv'],
          },
        },
      },
    },
  }
})