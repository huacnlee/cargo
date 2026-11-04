import { defineConfig } from 'astro/config';
export default defineConfig({
  site: process.env.SITE_URL || 'https://huacnlee.github.io',
  base: process.env.BASE_PATH || '/cargo',
  trailingSlash: 'always',
  devToolbar: { enabled: false },
});
