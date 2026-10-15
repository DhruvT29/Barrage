import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'

// The page is built into the engine, which serves it (`cargo run --bin barrage`).
// In development, `pnpm dev` serves it here and passes /api to a running engine.
export default defineConfig({
  plugins: [react()],
  build: {
    outDir: '../engine/static',
    emptyOutDir: true,
  },
  server: {
    proxy: {
      // Never changeOrigin: the engine answers only when Host and Origin match,
      // which they do only while the proxy passes the browser's own Host on.
      '/api': { target: 'http://127.0.0.1:7700', ws: true },
    },
  },
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test-setup.ts'],
  },
})
