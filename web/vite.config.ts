import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// The daemon serves this bundle at /ui (src/bootstrap/web_ui.rs), so every
// asset URL is built under that base. `pnpm dev` proxies the API to a
// daemon on the default port, so the dev server is same-origin too.
export default defineConfig({
  base: "/ui/",
  plugins: [react(), tailwindcss()],
  server: {
    proxy: {
      "/v1": "http://127.0.0.1:7070",
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    // The CSP allows no inline script; keep Vite from inlining small
    // assets as data: URLs into script, and keep the bundle inspectable.
    assetsInlineLimit: 0,
    sourcemap: false,
  },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    restoreMocks: true,
  },
});
