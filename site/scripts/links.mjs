// Dead-link check, run after every build: every internal href/src in dist/ must point at a built
// file, and every #anchor at an id on that page. It also checks the docs links Studio opens
// (DOCS in studio/src/config.ts). External links aren't fetched. Exit 1 lists what's broken.
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const dist = join(here, '..', 'dist');
const pages = [];
const walk = (d) => readdirSync(d).forEach((f) => (statSync(join(d, f)).isDirectory() ? walk(join(d, f)) : f.endsWith('.html') && pages.push(join(d, f))));
walk(dist);

const ids = new Map();
const idsOf = (file) => {
  if (!ids.has(file)) ids.set(file, new Set([...readFileSync(file, 'utf8').matchAll(/\sid="([^"]+)"/g)].map((m) => m[1])));
  return ids.get(file);
};
/** The built file for a site path, or null. */
const fileFor = (path) => {
  const p = decodeURIComponent(path.replace(/\/$/, '')) || '/';
  for (const c of [join(dist, p), join(dist, p, 'index.html'), join(dist, `${p}.html`)]) if (existsSync(c) && statSync(c).isFile()) return c;
  return null;
};

const broken = [];
const check = (from, link) => {
  if (!link.startsWith('/') || link.startsWith('//')) return;
  const [path, hash] = link.split('#');
  const file = path ? fileFor(path.split('?')[0]) : from;
  if (!file) return broken.push(`${from ? from.slice(dist.length) : 'Studio'}: ${link} (no such page)`);
  if (hash && file.endsWith('.html') && !idsOf(file).has(hash)) broken.push(`${from ? from.slice(dist.length) : 'Studio'}: ${link} (no #${hash} on that page)`);
};

let n = 0;
for (const page of pages) {
  const html = readFileSync(page, 'utf8');
  for (const m of html.matchAll(/\s(?:href|src)="([^"]+)"/g)) {
    const link = m[1].replace(/&amp;/g, '&');
    if (link.startsWith('#')) {
      n++;
      if (link.length > 1 && !idsOf(page).has(link.slice(1))) broken.push(`${page.slice(dist.length)}: ${link} (no such id)`);
    } else if (link.startsWith('/') && !link.startsWith('//')) {
      n++;
      check(page, link);
    }
  }
}

// Studio's "How this works" links: `${SITE_URL}/docs/...` in its config.
const studio = readFileSync(join(here, '..', '..', 'studio', 'src', 'config.ts'), 'utf8');
for (const m of studio.matchAll(/\$\{SITE_URL\}(\/docs[^`]*)`/g)) {
  n++;
  check(null, m[1]);
}

if (broken.length) {
  console.error(`${broken.length} dead link${broken.length > 1 ? 's' : ''} (of ${n} checked):\n  ${broken.join('\n  ')}`);
  process.exit(1);
}
console.log(`links: ${n} internal links and anchors checked across ${pages.length} pages, 0 dead`);
