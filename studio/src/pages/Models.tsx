// Models: the library as a quiet list, judged against this machine. A few filters on the left (the
// rest fold under "More filters"), one row per model, details in a panel on the right. Custom models
// come in through "Add a model": the engine's compatibility check, step by step.

import { DOCS } from '../config';
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { api, cancelPull, deleteModel, discardPaused, fail, pullModels, refresh, store, useApp } from '../app/state';
import type { Model, PullProgress, Task } from '../api/types';
import { CustomBadge } from '../components/ModelSwitcher';
import { AddModel, MODEL_EXTS } from '../components/AddModel';
import { Button, ConfirmButton, ConfirmLink, cx, HowLink, Icon, IconButton, Notice, Progress, StatusDot, useNow } from '../components/ui';
import { activeFilters, cardsOf, countWith, filterCards, isClean, isInstalled, largest, NO_FILTERS, SIZES, smallest, TAG_LABELS, type Card, type Filters, type Sort } from '../lib/catalog';
import { bytes, eta, pausedLabel, pullTotals, shortGpu, waitLabel } from '../lib/format';
import { canDo, fitFor, memoryLabel, memoryShort, seconds, TASKS, tasksOf, timeHere, timingFor, timingLabel } from '../lib/models';
import { onDesktopDrop, openExternal } from '../lib/tauri';

const FILTERS_KEY = 'studio.models.filters';
const loadFilters = (): Filters => {
  try {
    return { ...NO_FILTERS, ...JSON.parse(sessionStorage.getItem(FILTERS_KEY) ?? '{}') };
  } catch {
    return NO_FILTERS;
  }
};

/** The row grid: model · memory here · time · download · state. */
const COLS = 'grid grid-cols-[minmax(0,1fr)_200px_110px_110px_120px] items-center gap-x-5';
/** With the details panel open the list is narrower: model · memory · state (time and size are in the panel). */
const COLS_NARROW = 'grid grid-cols-[minmax(0,1fr)_170px_100px] items-center gap-x-4';

export function Models() {
  const models = useApp((s) => s.models);
  const families = useApp((s) => s.families);
  const gpu = useApp((s) => s.gpu);
  const [f, setF] = useState<Filters>(loadFilters);
  const [open, setOpen] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [dropped, setDropped] = useState<string | null>(null);
  const [droppedPath, setDroppedPath] = useState<string | null>(null);
  // In the desktop app a model file dropped anywhere on Models comes with its path: Add a model checks it.
  useEffect(
    () =>
      onDesktopDrop({
        drop: (paths) => {
          const p = paths.find((x) => MODEL_EXTS.test(x));
          if (p) (setDroppedPath(p), setDropped(null), setAdding(true));
        },
      }),
    [],
  );
  const [sheet, setSheet] = useState<Model | null>(null);
  const [more, setMore] = useState(() => !!(f.size || f.licence || f.tags.length));

  useEffect(() => {
    try {
      sessionStorage.setItem(FILTERS_KEY, JSON.stringify(f));
    } catch {
      // private mode
    }
  }, [f]);

  const cards = useMemo(() => cardsOf(models, families), [models, families]);
  const shown = useMemo(() => filterCards(cards, f), [cards, f]);
  const set = (p: Partial<Filters>) => setF((x) => ({ ...x, ...p }));
  const n = (p: Partial<Filters>) => countWith(cards, f, p);
  const current = cards.find((c) => c.id === open) ?? null;
  const tags = [...new Set(cards.flatMap((c) => c.shown.tags))].filter((t) => TAG_LABELS[t]);
  const hidden = [f.size, f.licence].filter(Boolean).length + f.tags.length;

  // Best-for-this-machine order reads as two shelves: what's here, then the rest of the library.
  // Custom models sit on their own shelf first: the user's additions, never mixed into the library.
  const mine = (c: Card) => !!c.shown.custom;
  const shelves =
    f.sort === 'best' && f.show === 'all'
      ? [{ title: 'Your models', rows: shown.filter(mine) }, { title: 'On this machine', rows: shown.filter((c) => !mine(c) && isInstalled(c)) }, { title: 'In the library', rows: shown.filter((c) => !mine(c) && !isInstalled(c)) }]
      : [{ title: '', rows: shown }];

  const download = (m: Model) => {
    if (m.licence.accept_required && m.licence_accepted_ms == null) setSheet(m);
    else pullModels([m.id]);
  };

  return (
    <div
      className="flex min-h-0 min-w-0 flex-1"
      onDragOver={(e) => [...e.dataTransfer.types].includes('Files') && e.preventDefault()}
      onDrop={(e) => {
        // A model file dropped anywhere on Models opens "Add a model" (the browser can't give Studio its path).
        const file = Array.from(e.dataTransfer.files).find((x) => MODEL_EXTS.test(x.name));
        if (!file) return;
        e.preventDefault();
        setDropped(file.name);
        setAdding(true);
      }}
    >
      <aside aria-label="Filters" className="flex w-[220px] flex-none flex-col gap-6 overflow-y-auto border-r border-line px-4 pt-5 pb-6">
        <label className="flex h-9 items-center gap-2 rounded-control border border-line bg-surface px-2.5 text-muted focus-within:border-text/30">
          <Icon name="search" size={15} />
          <input type="search" value={f.q} onChange={(e) => set({ q: e.target.value })} placeholder="Search" aria-label="Search models" className="selectable min-w-0 flex-1 bg-transparent text-[13px] text-text outline-none placeholder:text-muted" />
        </label>

        <label className="flex cursor-pointer items-center justify-between gap-3 text-[13px] text-text" title={gpu ? `${shortGpu(gpu.name)}, ${Math.round(gpu.vram_total_mib / 1024)} GB` : undefined}>
          Runs on this machine
          <input type="checkbox" role="switch" checked={f.runsHere} onChange={(e) => set({ runsHere: e.target.checked })} className="peer sr-only" />
          <span aria-hidden="true" className={cx('relative h-[18px] w-8 flex-none rounded-full transition-colors peer-focus-visible:outline-2 peer-focus-visible:outline-ring', f.runsHere ? 'bg-text' : 'bg-line-strong')}>
            <span className={cx('absolute top-[2px] size-[14px] rounded-full bg-bg transition-[left]', f.runsHere ? 'left-[16px]' : 'left-[2px]')} />
          </span>
        </label>

        <Options title="Show" value={f.show} onChange={(show) => set({ show })} options={[
          { v: 'all', label: 'Everything', n: n({ show: 'all' }) },
          { v: 'installed', label: 'Installed', n: n({ show: 'installed' }) },
          { v: 'available', label: 'Not downloaded', n: n({ show: 'available' }) },
        ]} />
        <Options title="Task" value={f.task} onChange={(task) => set({ task })} options={[{ v: '' as const, label: 'Any', n: n({ task: '' }) }, ...TASKS.map((t) => ({ v: t.id, label: t.label, n: n({ task: t.id }) }))]} />

        <div className="flex flex-col gap-5">
          <button type="button" aria-expanded={more} onClick={() => setMore((m) => !m)} className="flex items-center justify-between text-[12.5px] text-muted hover:text-text">
            <span>
              More filters{hidden > 0 && <span className="edge ml-1.5 text-text">{hidden}</span>}
            </span>
            <span className={cx('transition-transform', more && 'rotate-180')}>
              <Icon name="chevron" size={14} />
            </span>
          </button>
          {more && (
            <>
              <Options title="Download size" value={f.size} onChange={(size) => set({ size })} options={[{ v: '' as const, label: 'Any', n: n({ size: '' }) }, ...SIZES.map((s) => ({ v: s.id, label: s.label, n: n({ size: s.id }) }))]} />
              <Options title="Licence" value={f.licence} onChange={(licence) => set({ licence })} options={[
                { v: '', label: 'Any', n: n({ licence: '' }) },
                { v: 'commercial', label: 'Commercial OK', n: n({ licence: 'commercial' }) },
                { v: 'restricted', label: 'With restrictions', n: n({ licence: 'restricted' }) },
              ]} />
              {tags.length > 0 && (
                <section className="flex flex-col gap-0.5">
                  <h2 className="eyebrow m-0 pb-1.5 text-muted">Good at</h2>
                  {tags.map((t) => {
                    const on = f.tags.includes(t);
                    return <OptionRow key={t} checkbox on={on} label={TAG_LABELS[t]} n={n({ tags: on ? f.tags : [...f.tags, t] })} onClick={() => set({ tags: on ? f.tags.filter((x) => x !== t) : [...f.tags, t] })} />;
                  })}
                </section>
              )}
            </>
          )}
        </div>
      </aside>

      <main className="enter flex min-w-0 flex-1 flex-col overflow-y-auto">
        <header className="flex items-center gap-4 px-8 pt-6 pb-4">
          <h1 className="m-0 text-[22px] font-semibold tracking-[-0.01em]">Models</h1>
          {activeFilters(f) > 0 && (
            <button type="button" onClick={() => setF({ ...NO_FILTERS, sort: f.sort })} className="flex items-center gap-1 rounded-full border border-line px-2.5 py-0.5 text-[12px] text-muted hover:text-text">
              {shown.length} of {cards.length}
              <Icon name="close" size={11} />
            </button>
          )}
          <div className="flex-1" />
          <select aria-label="Sort" value={f.sort} onChange={(e) => set({ sort: e.target.value as Sort })} className="h-8 rounded-control border border-line bg-bg px-2 text-[12.5px] text-muted hover:text-text">
            <option value="best">Best for this machine</option>
            <option value="smallest">Smallest first</option>
            <option value="largest">Largest first</option>
            <option value="name">Name</option>
          </select>
          <Button size="sm" icon="plus" onClick={() => setAdding(true)}>
            Add a model
          </Button>
        </header>

        <div className="flex flex-col gap-8 px-8 pb-10">
          {shelves
            .filter((s) => s.rows.length)
            .map((s) => (
              <section key={s.title || 'all'} className="flex flex-col">
                <div className={cx(current ? COLS_NARROW : COLS, 'border-b border-line pb-2 text-[11.5px] text-muted')}>
                  <span className="eyebrow truncate">{s.title ? `${s.title} · ${s.rows.length}` : `${s.rows.length} models`}</span>
                  <span>Memory here</span>
                  {!current && <span>Time</span>}
                  {!current && <span>Download</span>}
                  <span />
                </div>
                {s.rows.map((c) => (
                  <ModelRow key={c.id} card={c} task={f.task} narrow={!!current} selected={open === c.id} onOpen={() => setOpen(open === c.id ? null : c.id)} onDownload={download} />
                ))}
              </section>
            ))}
          {shown.length === 0 && (
            <div className="flex flex-col items-center gap-3 py-20 text-[13.5px] text-muted">
              Nothing matches.
              <Button size="sm" onClick={() => setF({ ...NO_FILTERS, sort: f.sort })}>
                Clear filters
              </Button>
            </div>
          )}
        </div>
      </main>

      {current && <Details key={current.id} card={current} task={f.task} onClose={() => setOpen(null)} onDownload={download} />}
      {adding && <AddModel dropped={dropped} droppedPath={droppedPath} onClose={() => (setAdding(false), setDropped(null), setDroppedPath(null))} onShow={(id) => (setAdding(false), setOpen(families.find((x) => x.variants.includes(id))?.id ?? id))} />}
      {sheet && <LicenceSheet model={sheet} onClose={() => setSheet(null)} />}
    </div>
  );
}

// ---------- Sidebar pieces ----------

function OptionRow({ on, label, n, onClick, checkbox }: { on: boolean; label: string; n: number; onClick: () => void; checkbox?: boolean }) {
  return (
    <button type="button" role={checkbox ? 'checkbox' : 'radio'} aria-checked={on} onClick={onClick} className={cx('flex h-7 items-center gap-2.5 rounded-control px-1.5 text-left text-[13px] transition-colors hover:bg-surface', on ? 'text-text' : 'text-muted', n === 0 && !on && 'opacity-45')}>
      <span className={cx('flex size-[12px] flex-none items-center justify-center border-[1.5px]', checkbox ? 'rounded-[3px]' : 'rounded-full', on ? 'border-text bg-text text-bg' : 'border-line-strong')}>
        {on && (checkbox ? <Icon name="check" size={9} stroke={2.5} /> : <span className="size-[4px] rounded-full bg-bg" />)}
      </span>
      <span className="flex-1">{label}</span>
      <span className="edge text-[11px] text-muted">{n}</span>
    </button>
  );
}

function Options<T extends string>({ title, value, onChange, options }: { title: string; value: T; onChange: (v: T) => void; options: { v: T; label: string; n: number }[] }) {
  return (
    <section role="radiogroup" aria-label={title} className="flex flex-col gap-0.5">
      <h2 className="eyebrow m-0 pb-1.5 text-muted">{title}</h2>
      {options.map((o) => (
        <OptionRow key={o.v} on={o.v === value} label={o.label} n={o.n} onClick={() => onChange(o.v)} />
      ))}
    </section>
  );
}

// ---------- One row per model ----------

const mainTask = (m: Model, task: '' | Task): Task => task || (canDo(m, 'generate') ? 'generate' : 'edit');

function stateLine(m: Model, pull: PullProgress | null, queued: boolean): { text: string; tone: 'ok' | 'busy' | 'off' | 'warn' } | null {
  if (m.state === 'pulling') return { text: pull?.stage === 'testing' ? 'Testing…' : `${Math.round(pullTotals(m, pull).fraction * 100)}%`, tone: 'busy' };
  if (queued) return { text: 'Next', tone: 'busy' };
  if (m.state === 'installed') return { text: m.loaded ? 'Loaded' : 'Installed', tone: 'ok' };
  if (m.state === 'needs_test') return { text: 'Needs test', tone: 'warn' };
  if (m.state === 'test_failed') return { text: 'Test failed', tone: 'warn' };
  if (m.paused || m.state === 'partial') return { text: 'Paused', tone: 'warn' };
  return null;
}

function ModelRow({ card, task, narrow, selected, onOpen, onDownload }: { card: Card; task: '' | Task; narrow: boolean; selected: boolean; onOpen: () => void; onDownload: (m: Model) => void }) {
  const pulls = useApp((s) => s.pulls);
  const queue = useApp((s) => s.pullQueue);
  const m = card.shown;
  const t = mainTask(m, task);
  const timing = timingFor(m, t);
  const speed = timeHere(timing);
  const fit = fitFor(m, t);
  const status = stateLine(m, pulls[m.id] ?? m.pull, queue.includes(m.id));
  const size = card.variants.length > 1 ? `${bytes(smallest(card))}–${bytes(largest(card))}` : bytes(m.size_bytes);
  const facts = [tasksOf(m).length > 1 ? 'Generate + Edit' : tasksOf(m)[0] === 'edit' ? 'Edit' : 'Generate', m.custom ? 'Custom' : null, isClean(m) ? null : m.licence.spdx === 'NOASSERTION' ? 'no licence stated' : m.licence.commercial ? 'use restrictions' : 'non-commercial'].filter(Boolean).join(' · ');
  return (
    <div
      role="button"
      tabIndex={0}
      aria-expanded={selected}
      onClick={onOpen}
      onKeyDown={(e) => (e.key === 'Enter' || e.key === ' ') && (e.preventDefault(), onOpen())}
      className={cx(narrow ? COLS_NARROW : COLS, 'group min-h-[58px] cursor-pointer border-b border-line py-2.5 transition-colors', selected ? 'bg-surface' : 'hover:bg-surface/60')}
      data-model={m.family}
    >
      <span className="flex min-w-0 flex-col gap-0.5 pl-2">
        <span className="flex items-baseline gap-2">
          <span className="truncate text-[14px] font-medium">{m.name}</span>
          <span className="edge flex-none text-[11px] text-muted">{card.variants.map((v) => v.tag).filter(Boolean).join(' · ')}</span>
        </span>
        <span className="truncate text-[12px] text-muted">{facts}</span>
      </span>
      <span className="flex min-w-0 items-center gap-2 text-[12.5px]" title={memoryLabel(fit)}>
        <StatusDot state={fit.tier === 'fits' ? 'ok' : fit.tier === 'offload' ? 'warn' : 'off'} />
        <span className={cx('edge truncate', fit.tier === 'fits' || fit.tier === 'offload' ? 'text-text' : 'text-muted')}>{memoryShort(fit)}</span>
      </span>
      {!narrow && <span className="edge text-[12.5px] text-text">{speed != null ? seconds(speed) : ''}</span>}
      {!narrow && <span className="edge text-[12.5px] text-muted">{size}</span>}
      <span className="flex justify-end pr-2">
        {status ? (
          <span className="flex items-center gap-1.5 text-[12.5px] text-muted">
            <StatusDot state={status.tone} />
            {status.text}
          </span>
        ) : (
          <button
            type="button"
            onClick={(e) => (e.stopPropagation(), onDownload(m))}
            className="flex h-7 items-center gap-1.5 rounded-control border border-line px-2.5 text-[12px] text-muted opacity-0 transition-opacity group-focus-within:opacity-100 group-hover:opacity-100 hover:text-text focus-visible:opacity-100"
          >
            <Icon name="download" size={13} />
            Get
          </button>
        )}
      </span>
    </div>
  );
}

// ---------- Details: everything about one model, and what to do with it ----------

function Details({ card, task, onClose, onDownload }: { card: Card; task: '' | Task; onClose: () => void; onDownload: (m: Model) => void }) {
  const pulls = useApp((s) => s.pulls);
  const queue = useApp((s) => s.pullQueue);
  const models = useApp((s) => s.models);
  const [picked, setPicked] = useState(card.shown.id);
  const now = useNow();
  const m = card.variants.find((v) => v.id === picked) ?? card.shown;
  const progress = pulls[m.id] ?? m.pull;
  const pulling = m.state === 'pulling';
  const queued = queue.includes(m.id) && !pulling;
  const totals = pullTotals(m, progress);
  const wait = progress?.stage === 'waiting' && progress.wait ? progress.wait : null;
  const gate = m.licence.accept_required && m.licence_accepted_ms == null;
  const left = m.download_bytes || m.size_bytes;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    addEventListener('keydown', onKey);
    return () => removeEventListener('keydown', onKey);
  }, [onClose]);

  // Which other installed models share a file: those files are "already on disk" and kept on delete.
  const sharedWith = (path: string) => models.filter((x) => x.id !== m.id && x.state === 'installed' && x.files.some((f) => f.path === path)).map((x) => x.name);

  return (
    <aside aria-label={`${m.name} details`} className="enter flex w-[360px] flex-none flex-col overflow-y-auto border-l border-line bg-bg">
      <div className="sticky top-0 z-10 flex items-start justify-between gap-3 border-b border-line bg-bg px-5 pt-4 pb-3.5">
        <div className="flex min-w-0 flex-col gap-1">
          <h2 className="m-0 text-[18px] leading-tight font-semibold">{m.name}</h2>
          <span className="edge truncate text-muted">{m.custom?.source ?? m.original ?? m.family}</span>
        </div>
        <IconButton icon="close" label="Close details" size={30} onClick={onClose} className="border-transparent" />
      </div>
      <div className="flex flex-col gap-5 px-5 pt-4 pb-6">
        <p className="m-0 text-[13px] leading-relaxed text-muted">{m.custom ? (m.label ?? 'Custom, not reviewed by Dawnroom') : m.tagline}</p>

        {/* The size tags: each with its size, how it fits here, and whether it's on disk. */}
        {card.variants.length > 1 && (
          <DSection title="Size" docs={DOCS.sizeTags}>
            <div role="radiogroup" aria-label="Size tag" className="flex flex-col gap-1">
              {card.variants.map((v) => {
                const fit = fitFor(v, mainTask(v, task));
                const on = v.id === m.id;
                return (
                  <button key={v.id} type="button" role="radio" aria-checked={on} onClick={() => setPicked(v.id)} className={cx('flex items-center gap-2.5 rounded-control border px-2.5 py-2 text-left transition-colors', on ? 'border-line-strong bg-surface' : 'border-transparent hover:bg-surface')}>
                    <span className={cx('flex size-[11px] flex-none items-center justify-center rounded-full border-[1.5px]', on ? 'border-text' : 'border-muted')}>{on && <span className="size-[5px] rounded-full bg-text" />}</span>
                    <span className="edge w-12 text-text">{v.tag}</span>
                    <span className="edge w-16 text-muted">{bytes(v.size_bytes)}</span>
                    <span className="edge flex-1 truncate text-muted" title={memoryLabel(fit)}>{memoryShort(fit)}</span>
                    {v.state === 'installed' && <Icon name="check" size={13} />}
                  </button>
                );
              })}
            </div>
          </DSection>
        )}

        {/* The one action, and its progress. */}
        <div className="flex flex-col gap-2">
          {pulling ? (
            <>
              <Progress value={totals.fraction} label={`${m.name} download`} />
              <div className="edge flex justify-between gap-4 text-muted" aria-live="polite">
                <span>
                  {progress?.stage === 'testing' ? 'Making its test image…' : progress?.stage === 'verifying' ? 'Checking the download…' : wait ? waitLabel(wait, now) : `${Math.round(totals.fraction * 100)}% · ${bytes(totals.done)} of ${bytes(totals.total)}`}
                </span>
                <span>{progress?.stage === 'downloading' && progress.speed_bps > 0 ? eta((totals.total - totals.done) / progress.speed_bps) : ''}</span>
              </div>
              <Button size="sm" icon="stop" className="self-start" onClick={() => void cancelPull(m.id)}>
                Pause download
              </Button>
            </>
          ) : queued ? (
            <Button size="sm" className="self-start" onClick={() => store.set((s) => ({ ...s, pullQueue: s.pullQueue.filter((x) => x !== m.id) }))}>
              Remove from the queue
            </Button>
          ) : m.state === 'installed' ? (
            <div className="flex items-center justify-between gap-3">
              <span className="flex items-center gap-2 text-[13px]">
                <StatusDot state="ok" />
                Installed{m.loaded ? ' · loaded on the GPU' : ''}
              </span>
              <ConfirmButton label="Delete" confirm={`Delete ${bytes(m.size_bytes)}?`} onConfirm={() => void deleteModel(m.id)} />
            </div>
          ) : m.state === 'verifying' ? (
            <span className="edge text-muted">Checking files…</span>
          ) : (
            <>
              <Button icon="download" onClick={() => onDownload(m)} className="self-start border-line-strong">
                {m.state === 'needs_test' || m.state === 'test_failed' ? 'Make the test image' : m.paused || m.state === 'partial' ? `Resume · ${bytes(left)} left` : `Download · ${bytes(left)}`}
              </Button>
              {gate && <span className="text-[12px] text-muted">You'll be asked to accept its licence first.</span>}
              {m.paused && (
                <span className="flex flex-wrap gap-x-3 text-[12px] text-muted">
                  {pausedLabel(m.paused)}
                  <ConfirmLink label="Discard" confirm="Discard it? It starts again from zero." onConfirm={() => void discardPaused(m.id)} />
                </span>
              )}
            </>
          )}
          {m.pull_error && m.pull_error.code !== 'cancelled' && !pulling && <Notice>{m.pull_error.message}</Notice>}
        </div>

        <DSection title="On this machine" docs={DOCS.offload}>
          {tasksOf(m).map((t) => {
            const fit = fitFor(m, t);
            return (
              <div key={t} className="flex flex-col gap-1 rounded-control border border-line px-3 py-2.5">
                <div className="flex items-center justify-between gap-2">
                  <span className="text-[13px] font-medium">{t === 'edit' ? 'Editing' : 'Making images'}</span>
                  {m.custom && <CustomBadge />}
                </div>
                <span className="flex items-center gap-2 text-[12.5px]">
                  <StatusDot state={fit.tier === 'fits' ? 'ok' : fit.tier === 'offload' ? 'warn' : 'off'} />
                  {memoryLabel(fit)}
                </span>
                {timingLabel(timingFor(m, t)) && <span className="edge text-muted">{timingLabel(timingFor(m, t))}</span>}
              </div>
            );
          })}
        </DSection>

        <DSection title="Licence" docs={DOCS.licences}>
          <p className="m-0 text-[13px] text-text">{m.licence.name ?? m.licence.spdx}</p>
          <p className="m-0 text-[12.5px] leading-snug text-muted">
            {isClean(m) ? 'Commercial use allowed.' : m.licence.commercial ? 'Commercial use allowed, with use-based restrictions.' : 'Not for commercial use.'}
            {m.licence.accept_required && (m.licence_accepted_ms ? ' You accepted it here.' : ' Its official repo asks you to accept it before downloading.')}
          </p>
          {m.licence.note && <p className="m-0 text-[12.5px] leading-snug text-text">{m.licence.note}</p>}
          {m.licence.url && (
            <button type="button" onClick={() => openExternal(m.licence.url)} className="flex items-center gap-1.5 self-start rounded-[4px] text-[12.5px] text-link underline underline-offset-2">
              Read the licence <Icon name="open" size={12} />
            </button>
          )}
        </DSection>

        <DSection title="Files">
          <ul role="list" className="m-0 flex list-none flex-col gap-2 p-0">
            {m.files.map((file) => {
              const others = sharedWith(file.path);
              const here = file.status.state === 'verified' || file.status.state === 'present';
              return (
                <li key={file.path} className="flex flex-col gap-0.5">
                  <span className="flex items-baseline justify-between gap-3 text-[12.5px]">
                    <span className="edge truncate text-text" title={file.path}>
                      {file.path.split('/').pop()}
                    </span>
                    <span className="edge flex-none text-muted">{bytes(file.size)}</span>
                  </span>
                  <span className="edge text-[11px] text-muted">
                    {file.role.replace(/_/g, ' ')}
                    {here ? (others.length ? ` · already on disk, shared with ${others.join(', ')}` : ' · on disk') : ''}
                  </span>
                </li>
              );
            })}
          </ul>
          <p className="m-0 text-[12px] leading-snug text-muted">
            {m.custom
              ? `Added by you from ${m.custom.source}${m.custom.architecture ? `, read as ${m.custom.architecture}` : ''}. Not reviewed by Dawnroom.`
              : `From Hugging Face${m.original ? `; the original is ${m.original}` : ''}. Every file is pinned and checked against its sha256. Files shared with an installed model are kept when you delete this one.`}
          </p>
        </DSection>
      </div>
    </aside>
  );
}

function DSection({ title, docs, children }: { title: string; docs?: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-2">
      <div className="flex items-baseline justify-between gap-3">
        <h3 className="eyebrow m-0 text-muted">{title}</h3>
        {docs && <HowLink href={docs} className="text-[11.5px]" />}
      </div>
      {children}
    </section>
  );
}

/** Before the first download of a model whose official repo is gated: the user reads and accepts. */
export function LicenceSheet({ model: m, onClose, onAccepted }: { model: Model; onClose: () => void; onAccepted?: () => void }) {
  const ref = useRef<HTMLDialogElement>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    ref.current?.showModal();
  }, []);
  const name = m.licence.name ?? m.licence.spdx;
  const accept = async () => {
    setSaving(true);
    try {
      await api.acceptLicence(m.id);
      await refresh();
      if (onAccepted) onAccepted();
      else pullModels([m.id]);
      onClose();
    } catch (e) {
      fail(e);
      setSaving(false);
    }
  };
  return (
    <dialog ref={ref} onClose={onClose} aria-labelledby="lic-title" className="m-auto w-[440px] max-w-[calc(100vw-32px)] rounded-card border border-line-strong bg-surface p-0 text-text backdrop:bg-black/60">
      <div className="flex flex-col gap-3.5 px-5 py-4.5">
        <h2 id="lic-title" className="m-0 text-[16px] font-semibold">
          {m.name} has a licence to accept
        </h2>
        <p className="m-0 text-[13px] leading-relaxed text-muted">
          It's under the <span className="text-text">{name}</span>. Its official repo{m.original ? ` (${m.original})` : ''} asks people to accept that licence before downloading. Dawnroom downloads it from a mirror, and asks you the same.
        </p>
        {m.licence.note && <p className="m-0 text-[13px] leading-relaxed text-text">{m.licence.note}</p>}
        <button type="button" onClick={() => openExternal(m.licence.url)} className="flex items-center gap-1.5 self-start rounded-[4px] text-[13px] text-link underline underline-offset-2">
          Read the licence <Icon name="open" size={13} />
        </button>
        <HowLink href={DOCS.licences} label="Why Dawnroom asks" className="-mt-2 self-start" />
        <div className="mt-1 flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose} autoFocus>
            Cancel
          </Button>
          <Button icon="download" onClick={() => void accept()} disabled={saving} className="border-line-strong">
            Accept and download · {bytes(m.download_bytes || m.size_bytes)}
          </Button>
        </div>
      </div>
    </dialog>
  );
}


