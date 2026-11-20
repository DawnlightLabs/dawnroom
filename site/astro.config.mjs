import { defineConfig } from 'astro/config';
import tailwindcss from '@tailwindcss/vite';
import { SITE_URL } from './src/config.ts';

export default defineConfig({
  site: SITE_URL,
  // 4321 belongs to the Dawnlight landing; Dawnroom's site previews on 4322.
  server: { port: 4322 },
  vite: { plugins: [tailwindcss()] },
});
