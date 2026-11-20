import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  // The engine only accepts the origin http://localhost:5173 in dev, so never drift to another port.
  server: { port: 5173, strictPort: true, host: 'localhost' },
  build: { target: 'es2022' },
  test: { environment: 'node', include: ['src/**/*.test.ts'] },
});
