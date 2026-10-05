// The project's theme, served the way a Mendix deployment serves it: the
// stylesheet the model's tooling compiled (`assets/theme-cache/web/`) and
// what the theme and every module publish beside it (`assets/theme/web/`,
// `assets/themesource/<module>/public/`), all at the root of the site —
// where the stylesheet's own `url(...)`s expect its fonts and images.
import {
  cpSync,
  createReadStream,
  existsSync,
  readFileSync,
  readdirSync,
  statSync,
} from 'node:fs';
import { extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const assets = fileURLToPath(new URL('../../assets', import.meta.url));
const stylesheet = join(assets, 'theme-cache', 'web', 'theme.compiled.css');

const types = {
  '.css': 'text/css',
  '.eot': 'application/vnd.ms-fontobject',
  '.gif': 'image/gif',
  '.jpeg': 'image/jpeg',
  '.jpg': 'image/jpeg',
  '.js': 'text/javascript',
  '.json': 'application/json',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
  '.ttf': 'font/ttf',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
};

/** The folders a deployment merges into the site's root, the first one winning. */
function roots() {
  const found = [join(assets, 'theme-cache', 'web'), join(assets, 'theme', 'web')];
  const modules = join(assets, 'themesource');
  if (existsSync(modules)) {
    for (const module of readdirSync(modules).sort()) found.push(join(modules, module, 'public'));
  }
  return found.filter((root) => existsSync(root));
}

/** What a theme is written in, not what a browser asks for. */
const source = (file) => /\.(scss|map)$/.test(file) || file.endsWith('settings.json');

/**
 * What every module's `design-properties.json` declares, merged: the
 * properties of a widget type are those each module gives it, Atlas Core's
 * first.
 */
function designProperties() {
  const merged = {};
  const modules = join(assets, 'themesource');
  if (!existsSync(modules)) return merged;
  const names = readdirSync(modules).sort(
    (left, right) => (right === 'atlas_core') - (left === 'atlas_core') || left.localeCompare(right),
  );
  for (const module of names) {
    const file = join(modules, module, 'web', 'design-properties.json');
    if (!existsSync(file)) continue;
    let declared;
    try {
      declared = JSON.parse(readFileSync(file, 'utf8'));
    } catch {
      continue;
    }
    for (const [type, properties] of Object.entries(declared)) {
      if (Array.isArray(properties)) (merged[type] ??= []).push(...properties);
    }
  }
  return merged;
}

const DESIGN = 'virtual:mxrs-design-properties';

export function projectTheme() {
  let output = '';
  return {
    name: 'mxrs-project-theme',
    resolveId(id) {
      return id === DESIGN ? `\0${DESIGN}` : null;
    },
    load(id) {
      return id === `\0${DESIGN}` ? `export default ${JSON.stringify(designProperties())};` : null;
    },
    configResolved(config) {
      output = resolve(config.root, config.build.outDir);
    },
    transformIndexHtml() {
      if (!existsSync(stylesheet)) return [];
      return [
        { tag: 'link', attrs: { rel: 'stylesheet', href: './theme.compiled.css' }, injectTo: 'head' },
      ];
    },
    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        const path = decodeURIComponent((request.url || '').split('?')[0]);
        if (path === '/' || path.includes('..') || source(path)) return next();
        for (const root of roots()) {
          const file = join(root, path);
          if (existsSync(file) && statSync(file).isFile()) {
            response.setHeader('Content-Type', types[extname(file)] ?? 'application/octet-stream');
            createReadStream(file).pipe(response);
            return;
          }
        }
        next();
      });
    },
    closeBundle() {
      if (!existsSync(stylesheet) || !existsSync(output)) return;
      // Last first, so the first folder's file is the one that stays.
      for (const root of roots().reverse()) {
        cpSync(root, output, { recursive: true, filter: (file) => !source(file) });
      }
    },
  };
}
