import { cp, mkdir, readdir, rm } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const frontend = join(dirname(fileURLToPath(import.meta.url)), '..');
const source = join(frontend, 'dist');
const destination = join(frontend, '..', 'assets');

await mkdir(destination, { recursive: true });
for (const name of await readdir(destination)) {
  if (name === 'index.html' || name === '.vite' || /^app-.+\.(?:js|css)$/.test(name)) {
    await rm(join(destination, name), { recursive: true, force: true });
  }
}
await cp(source, destination, { recursive: true });
