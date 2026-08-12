import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

// Tauri hands the dev server a fixed port and expects it not to wander, so
// strictPort is on: silently moving to 5174 would leave the app pointing at nothing.
export default defineConfig({
  plugins: [solid()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    host: process.env.TAURI_DEV_HOST || false,
    hmr: process.env.TAURI_DEV_HOST
      ? { protocol: "ws", host: process.env.TAURI_DEV_HOST, port: 5174 }
      : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    // Android WebView trails desktop Chromium; es2020 is the floor that covers the
    // WebView versions still in circulation on devices people actually have.
    target: "es2020",
    minify: !process.env.TAURI_DEBUG,
    sourcemap: !!process.env.TAURI_DEBUG,
  },
});
