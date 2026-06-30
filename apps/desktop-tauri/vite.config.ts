import { fileURLToPath, URL } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite-plus";

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/ + Tauri conventions
export default defineConfig({
  plugins: [react(), tailwindcss()],

  resolve: {
    alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
  },

  // Tauri expects a fixed port and fails if it is not available.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: {
      // Don't watch the Rust backend.
      ignored: ["**/src-tauri/**"],
    },
  },

  // Workspace packages are shipped as TypeScript source; let Vite transpile them
  // instead of trying to pre-bundle them.
  optimizeDeps: {
    exclude: ["@qcksys/qlipq-core", "@qcksys/qlipq-ffmpeg"],
  },

  // es2022 is supported by every modern Tauri webview (WebView2 / recent
  // WKWebView) and avoids fragile downleveling for older targets.
  build: {
    target: "es2022",
    minify: !process.env.TAURI_ENV_DEBUG,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },

  fmt: {},
  lint: { options: { typeAware: false } },
});
