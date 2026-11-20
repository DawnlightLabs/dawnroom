// Dev fixture for trying first run and model downloads without pulling real weights.
//
// It copies the real registry into a scratch folder with small fake files (right names, wrong
// contents, matching sha256) and serves them slowly, the way Hugging Face would. Point a second
// engine at it:
//
//   node studio/scripts/first-run-fixture.mjs <scratch-dir>
//   dawnroom --port 8421 serve --data-dir <scratch-dir>/data --registry-dir <scratch-dir>/registry --hf-endpoint http://127.0.0.1:8431
//   open http://localhost:5173/?engine=http://127.0.0.1:8421
//
// The fake weights download and verify, but they can't make an image: the first job fails when
// sd-server tries to load them. That is expected.
//
// Faults, to see Studio's waiting state (the engine retries by itself):
//   curl http://127.0.0.1:8431/_fault/offline?secs=30   drop every connection for 30 s
//   curl http://127.0.0.1:8431/_fault/busy?n=3          answer the next 3 requests with 503

import { createHash } from 'node:crypto';
import { cpSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const out = process.argv[2];
if (!out) {
  console.error('usage: node first-run-fixture.mjs <scratch-dir> [port] [MB per second]');
  process.exit(1);
}
const port = Number(process.argv[3] ?? 8431);
const rate = Number(process.argv[4] ?? 6) * 1e6;
const registry = join(dirname(fileURLToPath(import.meta.url)), '..', '..', 'registry');

const BLOCK = 64 * 1024;
/** Fake file contents: one 64 kB block, different per file, repeated to the file's size. */
const block = (seed) => Buffer.from(Uint8Array.from({ length: BLOCK }, (_, i) => (i * 31 + seed * 7 + (i >> 8)) & 255));

function slice(seed, start, end) {
  const b = block(seed);
  const buf = Buffer.alloc(end - start);
  for (let o = 0; o < buf.length; ) o += b.copy(buf, o, (start + o) % BLOCK);
  return buf;
}

const files = new Map(); // url path → { seed, size }
/** Fake size by role: diffusion model, text encoders, everything else (VAE, tokenizer…). */
const sizeOf = (role) => (/diffusion|checkpoint/.test(role) ? 70e6 : /llm|t5|clip|encoder/.test(role) ? 40e6 : 8e6);
/** One fake per file on disk (category/filename): a file several models share stays one file, as in the real registry. */
const fakes = new Map(); // category/filename → { seed, size, sha256 }

mkdirSync(join(out, 'registry'), { recursive: true });
// Image runtimes are real (they come from GitHub, not Hugging Face): their manifests are copied as they are.
cpSync(join(registry, 'runtimes'), join(out, 'registry', 'runtimes'), { recursive: true });
let seed = 0;
for (const name of readdirSync(registry).filter((f) => f.endsWith('.toml'))) {
  // Shared files ([[files]]) and size tags' own files ([[variant.files]]) alike.
  const parts = readFileSync(join(registry, name), 'utf8').split(/(\[\[(?:variant\.)?files\]\])/);
  const faked = parts.map((text, i) => {
    if (i === 0 || i % 2 === 1) return text; // the manifest's head, and the section headers themselves
    const get = (key) => text.match(new RegExp(`^${key} = "(.*)"`, 'm'))?.[1] ?? '';
    const key = `${get('category')}/${get('filename')}`;
    if (!fakes.has(key)) {
      const id = ++seed;
      const size = sizeOf(get('role'));
      const hash = createHash('sha256');
      for (let at = 0; at < size; at += BLOCK) hash.update(slice(id, at, Math.min(size, at + BLOCK)));
      fakes.set(key, { seed: id, size, sha256: hash.digest('hex') });
    }
    const f = fakes.get(key);
    files.set(`/${get('repo')}/resolve/${get('revision')}/${get('path')}`, { seed: f.seed, size: f.size });
    return text.replace(/^size = .*$/m, `size = ${f.size}`).replace(/^sha256 = .*$/m, `sha256 = "${f.sha256}"`);
  });
  writeFileSync(join(out, 'registry', name), faked.join(''));
}

let offlineUntil = 0;
let busy = 0;

createServer(async (req, res) => {
  const url = new URL(req.url, 'http://x');
  if (url.pathname === '/_fault/offline') {
    offlineUntil = Date.now() + Number(url.searchParams.get('secs') ?? 30) * 1000;
    return res.end('offline');
  }
  if (url.pathname === '/_fault/busy') {
    busy = Number(url.searchParams.get('n') ?? 3);
    return res.end('busy');
  }
  if (Date.now() < offlineUntil) return req.socket.destroy();
  if (busy > 0) {
    busy--;
    return res.writeHead(503, { 'Retry-After': '5' }).end();
  }
  const file = files.get(decodeURI(req.url.split('?')[0]));
  if (!file) return res.writeHead(404).end('not in the fixture');
  // Resume support, as on Hugging Face: "Range: bytes=N-".
  const from = Number(/bytes=(\d+)-/.exec(req.headers.range ?? '')?.[1] ?? 0);
  res.writeHead(from ? 206 : 200, {
    'Content-Length': file.size - from,
    'Accept-Ranges': 'bytes',
    ...(from ? { 'Content-Range': `bytes ${from}-${file.size - 1}/${file.size}` } : {}),
  });
  const step = Math.round(rate / 10);
  for (let at = from; at < file.size && !res.destroyed; at += step) {
    if (Date.now() < offlineUntil) return req.socket.destroy();
    res.write(slice(file.seed, at, Math.min(file.size, at + step)));
    await new Promise((r) => setTimeout(r, 100));
  }
  res.end();
}).listen(port, '127.0.0.1', () => console.log(`fixture: ${files.size} fake files at http://127.0.0.1:${port}, ${rate / 1e6} MB/s; registry in ${join(out, 'registry')}`));
