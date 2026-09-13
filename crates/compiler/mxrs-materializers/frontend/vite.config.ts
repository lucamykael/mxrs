import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  base: './',
  plugins: [react()],
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    sourcemap: false,
    manifest: true,
    rollupOptions: { output: { entryFileNames: 'app-[hash].js', assetFileNames: 'app-[hash][extname]' } },
  },
});
