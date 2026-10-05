import { defineConfig, loadEnv } from 'vite';
import react from '@vitejs/plugin-react';

import { projectTheme } from './scripts/project-theme.mjs';

export default defineConfig(({ mode }) => {
  const environment = loadEnv(mode, '.', '');
  const runtime = environment.MXRS_API_ORIGIN
    || `http://127.0.0.1:${environment.MXRS_API_PORT || '8080'}`;
  return {
    base: './',
    plugins: [react(), projectTheme()],
    // `@/` is `src/`, read from tsconfig.json so the two never disagree.
    resolve: { tsconfigPaths: true },
    server: {
      // Development stays same-origin from the browser's point of view while
      // the Rust runtime remains a separately supervised process.
      proxy: {
        '/api': runtime,
        '/model.json': runtime,
      },
    },
    build: {
      outDir: 'dist',
      emptyOutDir: true,
      sourcemap: false,
      manifest: true,
      rollupOptions: { output: { entryFileNames: 'app-[hash].js', assetFileNames: 'app-[hash][extname]' } },
    },
  };
});
