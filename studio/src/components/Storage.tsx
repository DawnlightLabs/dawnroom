// Settings → Storage: what the data folder holds (from the engine, hardlinks counted once), the
// models on disk, paused downloads, Clear cache and moving the folder. The engine does all of it;
// this only shows the numbers and asks before anything is removed.

import { DOCS } from '../config';
import { useCallback, useEffect, useState } from 'react';
import type { Model, StorageReport } from '../api/types';
import { api, loadInfo, useApp } from '../app/state';
import { bytes } from '../lib/format';
import { clearWebCache, inTauri, pickFolder, reveal, webCacheInfo } from '../lib/tauri';
import { Button, ConfirmButton, cx, HowLink, Notice, Progress } from './ui';

/** Usage bar colours: Kit palette on ink, never colour alone (each has a labelled row). */
const KINDS = [
  { key: 'models', label: 'Models', color: 'var(--color-sky)' },
  { key: 'images', label: 'Your images', color: 'var(--color-safelight-to)' },
  { key: 'runtimes', label: 'Image runtimes', color: 'var(--color-safelight-from)' },
  { key: 'cache', label: 'Cache', color: 'var(--color-muted)' },
  { key: 'partial', label: 'Paused downloads', color: 'var(--color-ok)' },
  { key: 'other', label: 'Other files', color: 'var(--color-line-strong)' },
] as const;

const CACHE_LABEL: Record<StorageReport['cache'][number]['kind'], string> = {
  thumbnails: 'Thumbnails',
  rotated_log: 'Old log',
  old_runtime: 'Old image runtime',
  unpack_leftover: 'Unfinished runtime unpack',
  runtime_zip: 'Leftover runtime download',
};

function ago(ms: number | null): string {
  if (ms == null) return 'never used';
  const min = Math.max(0, Date.now() - ms) / 60000;
  if (min < 1) return 'used just now';
  if (min < 60) return `used ${Math.round(min)} min ago`;
  if (min < 48 * 60) return `used ${Math.round(min / 60)} h ago`;
  return `used ${Math.round(min / 1440)} days ago`;
}

export function StorageSection() {
  const models = useApp((s) => s.models);
  const [r, setR] = useState<StorageReport | null>(null);
  const [web, setWeb] = useState<{ bytes: number; scheduled: boolean } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const [moveTo, setMoveTo] = useState('');
  const [moving, setMoving] = useState(false);

  const load = useCallback(async () => {
    try {
      setR(await api.storage());
      setWeb(await webCacheInfo());
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);
  useEffect(() => void load(), [load]);

  // While a move runs: follow it, then wait for the engine to come back on the new folder.
  useEffect(() => {
    if (!moving) return;
    const from = r?.data_dir;
    const t = setInterval(async () => {
      try {
        const p = await api.moveStatus();
        setR((old) => (old ? { ...old, move: p } : old));
        if (p.state === 'failed') {
          setMoving(false);
          setError(p.error ?? 'The move failed. The old folder was not touched.');
        }
      } catch {
        // The engine is restarting; keep polling until it answers from the new folder.
      }
      try {
        const info = await api.info();
        if (from && info.data_dir !== from) {
          setMoving(false);
          setDone(`Moved. The data folder is now ${info.data_dir}.`);
          void loadInfo();
          void load();
        }
      } catch {
        /* still restarting */
      }
    }, 700);
    return () => clearInterval(t);
  }, [moving, r?.data_dir, load]);

  if (!r) return <p className="m-0 text-[13px] text-muted">{error ?? 'Reading the data folder…'}</p>;

  const free = r.drive?.free_bytes ?? 0;
  // Files that more than one installed model lists (encoders, VAEs): on disk once.
  const users = new Map<string, { size: number; n: number }>();
  for (const mo of models.filter((x) => x.state === 'installed')) for (const f of mo.files) users.set(f.path, { size: f.size, n: (users.get(f.path)?.n ?? 0) + 1 });
  const sharedFiles = [...users.entries()].filter(([, u]) => u.n > 1);
  const shared = { bytes: sharedFiles.reduce((t, [, u]) => t + u.size, 0), models: new Set(models.filter((x) => x.state === 'installed' && x.files.some((f) => sharedFiles.some(([p]) => p === f.path))).map((x) => x.id)).size };
  const whole = r.usage.total + free;
  const cacheTotal = r.usage.cache;
  const run = async (f: () => Promise<string>) => {
    setError(null);
    setDone(null);
    try {
      setDone(await f());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
    await load();
  };

  const startMove = async (to: string) => {
    setError(null);
    setDone(null);
    try {
      const p = await api.startMove(to);
      setR({ ...r, move: p });
      setMoving(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };
  const chooseFolder = async () => {
    const picked = await pickFolder('Move Dawnroom’s data to…');
    if (picked) setMoveTo(picked);
  };
  const m = r.move;
  const busyMove = moving && m.state !== 'failed';

  return (
    <div className="flex flex-col gap-4" data-storage>
      {/* The disk as a map: each model a block sized by what it takes, then your images, runtimes, cache. Shared files count once. */}
      <section className="flex flex-col gap-5 rounded-[16px] border border-line bg-surface p-6">
        <div className="flex flex-wrap items-end justify-between gap-4">
          <div className="flex items-baseline gap-8">
            <span className="flex flex-col">
              <span className="font-display text-[40px] leading-none font-light tracking-[-0.03em]">{bytes(r.usage.total)}</span>
              <span className="mt-2 text-[12.5px] text-muted">used by Dawnroom</span>
            </span>
            <span className="flex flex-col">
              <span className="font-display text-[40px] leading-none font-light tracking-[-0.03em] text-muted">{bytes(free)}</span>
              <span className="mt-2 text-[12.5px] text-muted">free on this drive</span>
            </span>
          </div>
          <Button size="sm" icon="folder" onClick={() => void reveal(r.data_dir)} className="flex-none" disabled={!inTauri} title={r.data_dir}>
            Open folder
          </Button>
        </div>
        {/* The whole drive, thin: how much of it is Dawnroom's. */}
        <div className="flex h-1.5 overflow-hidden rounded-full bg-line" role="img" aria-label={`${bytes(r.usage.total)} of ${bytes(whole)} on this drive`}>
          <div className="bg-safelight" style={{ width: `${Math.max(0.5, (r.usage.total / whole) * 100)}%` }} />
        </div>
        <DiskMapWithKey blocks={mapBlocks(r, models)} />
        <p className="m-0 text-[12.5px] leading-snug text-muted">
          {shared.bytes > 0 ? (
            <>
              <span className="text-text">{bytes(shared.bytes)} of files are shared</span> by {shared.models} models (text encoders and VAEs): stored once, and kept when you delete one of them.{' '}
            </>
          ) : null}
          Downloads check the free space first, keeping 500 MB spare.
        </p>
      </section>

      {/* Models on disk. */}
      <section className="flex flex-col gap-2.5 rounded-card border border-line bg-surface p-4">
        <h3 className="eyebrow m-0 text-muted">Models on disk</h3>
        {r.models.length === 0 && <p className="m-0 text-[13px] text-muted">None yet.</p>}
        {r.models.map((mo) => (
          <div key={mo.id} className="flex items-center justify-between gap-4 text-[13px]">
            <div className="flex min-w-0 flex-col">
              <span>
                {mo.name}
                {!mo.complete && <span className="text-muted"> · partly downloaded</span>}
              </span>
              <span className="edge text-muted">
                {bytes(mo.bytes)} · {ago(mo.last_used_ms)}
                {mo.shared.length > 0 && ` · shares ${mo.shared.length === 1 ? mo.shared[0].split('/').pop() : `${mo.shared.length} files`} with ${[...new Set(models.filter((x) => x.id !== mo.id && x.state === 'installed' && x.files.some((f) => mo.shared.includes(f.path))).map((x) => x.name))].join(', ') || 'another model'} (kept on delete)`}
              </span>
            </div>
            <ConfirmButton label="Delete" confirm={`Delete ${mo.name}?`} onConfirm={() => void run(async () => (await api.deleteModel(mo.id), `Deleted ${mo.name}.`))} className="flex-none" />
          </div>
        ))}
      </section>

      {/* Paused downloads: never touched by Clear cache. */}
      {r.partials.length > 0 && (
        <section className="flex flex-col gap-2.5 rounded-card border border-line bg-surface p-4">
          <h3 className="eyebrow m-0 text-muted">Paused downloads</h3>
          {r.partials.map((p) => (
            <div key={p.file} className="flex items-center justify-between gap-4 text-[13px]">
              <div className="flex min-w-0 flex-col">
                <span>{p.owner === 'runtime' ? 'Image runtime' : p.owner}</span>
                <span className="edge break-all text-muted">
                  {bytes(p.bytes)}
                  {p.total ? ` of ${bytes(p.total)}` : ''} · {p.file.split('/').pop()}
                </span>
              </div>
              <ConfirmButton label="Delete" confirm="Delete? It starts again from zero." onConfirm={() => void run(async () => `Freed ${bytes((await api.deletePartials({ file: p.file })).freed_bytes)}.`)} className="flex-none" />
            </div>
          ))}
          {r.partials.length > 1 && (
            <ConfirmButton label="Delete all paused downloads" confirm="Delete all of them?" onConfirm={() => void run(async () => `Freed ${bytes((await api.deletePartials({ all: true })).freed_bytes)}.`)} className="self-start" />
          )}
        </section>
      )}

      {/* Clear cache: only rebuildable things, listed before anything goes. */}
      <section className="flex flex-col gap-2.5 rounded-card border border-line bg-surface p-4">
        <h3 className="eyebrow m-0 text-muted">Cache</h3>
        <p className="m-0 text-[12.5px] leading-snug text-muted">Things Dawnroom can make again. Models, your images and paused downloads are never touched.</p>
        {r.cache.map((c) => (
          <div key={c.path} className="flex justify-between gap-4 text-[12.5px]">
            <span className="min-w-0 break-all text-muted">
              {CACHE_LABEL[c.kind]} <span className="edge">{c.path}</span>
            </span>
            <span className="edge flex-none">{bytes(c.bytes)}</span>
          </div>
        ))}
        {web && (
          <div className="flex justify-between gap-4 text-[12.5px]">
            <span className="text-muted">Studio's web cache{web.scheduled ? ' (cleared next time Studio starts)' : ''}</span>
            <span className="edge flex-none">{bytes(web.bytes)}</span>
          </div>
        )}
        {cacheTotal + (web?.bytes ?? 0) > 0 ? (
          <ConfirmButton
            label={`Clear cache · ${bytes(cacheTotal + (web && !web.scheduled ? web.bytes : 0))}`}
            confirm="Remove these?"
            action="Clear"
            icon="trash"
            onConfirm={() =>
              void run(async () => {
                const res = await api.clearCache();
                if (web && !web.scheduled) await clearWebCache();
                return `Freed ${bytes(res.freed_bytes)}${web && !web.scheduled ? '; the web cache clears next time Studio starts' : ''}.`;
              })
            }
            className="self-start"
          />
        ) : (
          <p className="m-0 text-[12.5px] text-muted">Nothing to clear.</p>
        )}
      </section>

      {/* Move the whole folder to another drive. */}
      <section className="flex flex-col gap-2.5 rounded-card border border-line bg-surface p-4">
        <div className="flex items-baseline justify-between gap-3">
          <h3 className="eyebrow m-0 text-muted">Move the data folder</h3>
          <HowLink href={DOCS.move} className="text-[11.5px]" />
        </div>
        <p className="m-0 text-[12.5px] leading-snug text-muted">Copies everything ({bytes(r.usage.total)}) to an empty folder, checks it, switches over and deletes the old copy. Wait for images and downloads to finish first.</p>
        {busyMove ? (
          <div className="flex flex-col gap-1.5">
            <Progress value={m.total ? m.bytes / m.total : 0} label="Moving the data folder" className="dawn-bar" />
            <span className="edge text-muted">{m.state === 'restarting' || m.state === 'switching' ? 'Switching over; the engine restarts on the new folder…' : `${m.state === 'verifying' ? 'Checking' : 'Copying'} · ${bytes(m.bytes)} of ${bytes(m.total)}`}</span>
          </div>
        ) : (
          <div className="flex items-center gap-2">
            <input
              value={moveTo}
              onChange={(e) => setMoveTo(e.target.value)}
              placeholder={inTauri ? 'Choose a folder…' : 'E:\\Dawnroom'}
              aria-label="New data folder"
              spellCheck={false}
              className="h-8 min-w-0 flex-1 rounded-control border border-line bg-bg px-2.5 font-mono text-[12.5px] text-text outline-none placeholder:text-muted focus-visible:border-line-strong"
            />
            {inTauri && (
              <Button size="sm" icon="folder" onClick={() => void chooseFolder()}>
                Choose…
              </Button>
            )}
            {moveTo.trim() && <ConfirmButton label="Move" icon="folder" confirm={`Move ${bytes(r.usage.total)} there?`} action="Move" onConfirm={() => void startMove(moveTo.trim())} className="flex-none" />}
          </div>
        )}
      </section>

      {error && <Notice>{error}</Notice>}
      {done && <Notice tone="info">{done}</Notice>}
    </div>
  );
}

// ---------- The disk map ----------

interface Block {
  key: string;
  label: string;
  bytes: number;
  color: string;
  /** Models get the stock colour band; the rest are flat. */
  kind: 'model' | 'other';
}

interface Placed extends Block {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** What goes on the map: each model on disk (scaled so models add up to their true, shared-once total), then the rest. */
function mapBlocks(r: StorageReport, models: Model[]): Block[] {
  const tones = ['#c47855', '#0e4490', '#59715c', '#a85b3b', '#2c5f8a', '#7d8f6e', '#8a5a44', '#3d6e9e'];
  const listed = r.models.filter((m) => m.bytes > 0);
  const sum = listed.reduce((t, m) => t + m.bytes, 0) || 1;
  const scale = Math.min(1, r.usage.models / sum);
  const out: Block[] = listed
    .sort((a, b) => b.bytes - a.bytes)
    .map((m, i) => ({ key: m.id, label: models.find((x) => x.id === m.id)?.name ?? m.name, bytes: m.bytes * scale, color: tones[i % tones.length], kind: 'model' }));
  for (const k of KINDS) if (k.key !== 'models' && r.usage[k.key] > 0) out.push({ key: k.key, label: k.label, bytes: r.usage[k.key], color: k.color, kind: 'other' });
  return out.filter((b) => b.bytes > 0);
}

/** Splits the list in two by size and the rectangle along its longer side, until each block has its own room. */
function layout(blocks: Block[], x: number, y: number, w: number, h: number): Placed[] {
  if (!blocks.length) return [];
  if (blocks.length === 1) return [{ ...blocks[0], x, y, w, h }];
  const total = blocks.reduce((t, b) => t + b.bytes, 0);
  let acc = 0;
  let cut = 1;
  for (let i = 0; i < blocks.length - 1; i++) {
    acc += blocks[i].bytes;
    cut = i + 1;
    if (acc >= total / 2) break;
  }
  const first = blocks.slice(0, cut);
  const share = first.reduce((t, b) => t + b.bytes, 0) / total;
  return w >= h
    ? [...layout(first, x, y, w * share, h), ...layout(blocks.slice(cut), x + w * share, y, w * (1 - share), h)]
    : [...layout(first, x, y, w, h * share), ...layout(blocks.slice(cut), x, y + h * share, w, h * (1 - share))];
}

function DiskMap({ blocks }: { blocks: Block[] }) {
  const placed = layout(blocks, 0, 0, 100, 100);
  return (
    <div className="relative h-[260px] overflow-hidden rounded-[10px]" role="list" aria-label="What takes the space">
      {placed.map((b) => {
        const roomy = b.w > 14 && b.h > 22;
        return (
          <div
            key={b.key}
            role="listitem"
            title={`${b.label}: ${bytes(b.bytes)}`}
            className={cx('absolute flex flex-col justify-end gap-0.5 overflow-hidden p-2.5', b.kind === 'model' ? 'text-[#fcf8f5]' : 'text-text')}
            style={{ left: `${b.x}%`, top: `${b.y}%`, width: `${b.w}%`, height: `${b.h}%`, background: b.kind === 'model' ? b.color : `color-mix(in srgb, ${b.color} 28%, var(--color-bg))`, boxShadow: 'inset 0 0 0 1.5px var(--color-surface)' }}
          >
            {roomy && (
              <>
                <span className="truncate text-[12.5px] leading-tight font-medium">{b.label}</span>
                <span className="edge text-[11px] opacity-80">{bytes(b.bytes)}</span>
              </>
            )}
          </div>
        );
      })}
    </div>
  );
}

/** The map with a key under it for the blocks too small to carry their own name. */
function DiskMapWithKey({ blocks }: { blocks: Block[] }) {
  const placed = layout(blocks, 0, 0, 100, 100);
  const small = placed.filter((b) => !(b.w > 14 && b.h > 22));
  return (
    <div className="flex flex-col gap-3">
      <DiskMap blocks={blocks} />
      {small.length > 0 && (
        <ul className="m-0 flex list-none flex-wrap gap-x-5 gap-y-1.5 p-0 text-[12px] text-muted">
          {small.map((b) => (
            <li key={b.key} className="flex items-center gap-1.5">
              <span aria-hidden="true" className="size-2 rounded-[2px]" style={{ background: b.kind === 'model' ? b.color : `color-mix(in srgb, ${b.color} 55%, var(--color-bg))` }} />
              {b.label}
              <span className="edge text-text">{bytes(b.bytes)}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
