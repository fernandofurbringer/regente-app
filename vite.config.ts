import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react()],

  // second entry: the phone PWA (mobile.html), served by the mobile server
  build: {
    // Monaco (lazy, ~2,7 MB) é o único chunk grande esperado; o resto fica abaixo de 1 MB (medido em U3)
    chunkSizeWarningLimit: 3000,
    rolldownOptions: {
      input: { main: "index.html", mobile: "mobile.html" },
      output: {
        // vendors em chunks próprios (Vite 8: manualChunks foi substituído por codeSplitting)
        codeSplitting: {
          groups: [
            { name: "vendor-react", entriesAware: true, test: /node_modules[\\/](react|react-dom|scheduler)[\\/]/ },
            { name: "vendor-xterm", entriesAware: true, test: /node_modules[\\/]@xterm[\\/]/ },
            { name: "vendor-markdown", entriesAware: true, test: /node_modules[\\/](react-markdown|remark-|rehype-|micromark|mdast-|hast-|unist-|unified|vfile|property-information|character-entities|decode-named-character-reference|bail|trough|zwitch|ccount|devlop|longest-streak|style-to-js|style-to-object|space-separated-tokens|comma-separated-tokens|html-void-elements|markdown-table|trim-lines|estree-util-)/ },
          ],
        },
      },
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
