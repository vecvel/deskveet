import { defineConfig } from "vite";

// Plain TS, no framework, no runtime deps. esbuild minifies; the bundle is a few KB.
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true, host: "127.0.0.1" },
  build: {
    target: "safari15",
    minify: "esbuild",
    cssMinify: true,
    sourcemap: false,
    reportCompressedSize: false,
    chunkSizeWarningLimit: 200,
    modulePreload: false,
  },
});
