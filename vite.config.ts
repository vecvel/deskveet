import { defineConfig } from "vite";

// Plain TS, no framework, no runtime deps. The default minifier is enough; the bundle is a few KB.
export default defineConfig({
  clearScreen: false,
  publicDir: false,
  server: { port: 1420, strictPort: true, host: "127.0.0.1" },
  build: {
    target: "safari15",
    sourcemap: false,
    reportCompressedSize: false,
    chunkSizeWarningLimit: 200,
    modulePreload: false,
  },
});
