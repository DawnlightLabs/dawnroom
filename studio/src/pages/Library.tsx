// Library: your prints, to look at rather than to manage. A cover-flow stage holds one print in the
// middle with the rest receding on either side (←/→, scroll, or click one); below it, every print as a
// quiet grid, grouped by sitting. Favourites is the stage alone, full height, on a lit wall. Any print
// opens full-bleed in the viewer.

import { useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import { api, deleteImage, editThis, remix, setFavourite, useApp, varyJob, view } from '../app/state';
import type { ImageItem } from '../api/types';
import { isAbort } from '../api/client';
import { GreasePencil } from '../components/Moments';
import { Button, ConfirmButton, CopyButton, cx, Icon } from '../components/ui';
import { duration, frameNo, jobMs, recipeText } from '../lib/format';
import { chainOf, clock, dayLabel, editsOf, sessionsOf } from '../lib/library';
import { reveal } from '../lib/tauri';

type Tab = 'all' | 'favourites';
const TAB_KEY = 'studio.library.tab';

/** Every image, a page at a time (the Library is a local folder: hundreds, not millions). */
async function loadAll(signal: AbortSignal): Promise<ImageItem[]> {
  const all: ImageItem[] = [];
  for (;;) {
    const page = await api.images({ limit: 200, offset: all.length }, signal);
    all.push(...page.items);
    if (!page.items.length || all.length >= page.total || all.length >= 5000) return all;
  }
}

export function Library() {
  const viewing = useApp((s) => s.viewing);
  const rev = useApp((s) => s.libraryRev);
  const models = useApp((s) => s.models);
  const [items, setItems] = useState<ImageItem[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [q, setQ] = useState('');
  const [searching, setSearching] = useState(false);
  const [tab, setTab] = useState<Tab>(() => {
    try {
      return sessionStorage.getItem(TAB_KEY) === 'favourites' ? 'favourites' : 'all';
    } catch {
      return 'all';
    }
  });
  const [focus, setFocus] = useState(0);
  const [justMarked, setJustMarked] = useState<string | null>(null);

  useEffect(() => {
    try {
      sessionStorage.setItem(TAB_KEY, tab);
    } catch {
      // private mode
    }
    setFocus(0);
  }, [tab]);

  useEffect(() => {
    const ctl = new AbortController();
    loadAll(ctl.signal).then(
      (all) => (setItems(all), setError(null)),
      (e) => !isAbort(e) && setError(e.message),
    );
    return () => ctl.abort();
  }, [rev]);

  const modelName = (id: string) => models.find((m) => m.id === id || m.family === id)?.name ?? id;
  const needle = q.trim().toLowerCase();
  const found = useMemo(() => (items ?? []).filter((i) => !needle || i.prompt.toLowerCase().includes(needle) || frameNo(i.frame).includes(needle)), [items, needle]);
  const list = tab === 'favourites' ? found.filter((i) => i.favourite) : found;
  useEffect(() => setFocus((f) => Math.min(f, Math.max(0, list.length - 1))), [list.length]);

  const toggleFavourite = (item: ImageItem) => {
    const favourite = !item.favourite;
    setItems((old) => old && old.map((i) => (i.id === item.id ? { ...i, favourite } : i)));
    setJustMarked(favourite ? item.id : null);
    void setFavourite(item.id, favourite);
  };
  const remove = async (item: ImageItem) => {
    const index = list.findIndex((i) => i.id === item.id);
    if (!(await deleteImage(item.id))) return;
    const rest = list.filter((i) => i.id !== item.id);
    setItems((old) => old && old.filter((i) => i.id !== item.id));
    view(rest[Math.min(index, rest.length - 1)]?.id ?? null);
  };

  const open = items?.find((i) => i.id === viewing);
  const favCount = items?.filter((i) => i.favourite).length ?? 0;

  return (
    <div className={cx('relative flex min-h-0 min-w-0 flex-1 flex-col', tab === 'favourites' && 'wall')}>
      <header className="relative z-10 grid flex-none grid-cols-[1fr_auto_1fr] items-center px-8 pt-5 pb-2">
        <h1 className="m-0 text-[22px] font-semibold tracking-[-0.01em]">Library</h1>
        <div role="tablist" aria-label="Library" className="flex rounded-full border border-line bg-surface/70 p-0.5 backdrop-blur-sm">
          {(
            [
              ['all', 'All prints'],
              ['favourites', 'Favourites'],
            ] as const
          ).map(([id, label]) => (
            <button key={id} type="button" role="tab" aria-selected={tab === id} onClick={() => setTab(id)} className={cx('h-7 rounded-full px-4 text-[13px] transition-colors', tab === id ? 'bg-text text-bg' : 'text-muted hover:text-text')}>
              {label}
              {id === 'favourites' && favCount > 0 && <span className="edge ml-1.5 opacity-60">{favCount}</span>}
            </button>
          ))}
        </div>
        <div className="flex justify-end">
          {searching || q ? (
            <label className="flex h-8 w-[220px] items-center gap-2 rounded-full border border-line bg-surface px-3 text-muted">
              <Icon name="search" size={14} />
              <input autoFocus type="search" value={q} onChange={(e) => setQ(e.target.value)} onBlur={() => setSearching(false)} onKeyDown={(e) => e.key === 'Escape' && (setQ(''), setSearching(false))} placeholder="Search prompts" aria-label="Search the Library" className="selectable min-w-0 flex-1 bg-transparent text-[13px] text-text outline-none placeholder:text-muted" />
            </label>
          ) : (
            <button type="button" aria-label="Search the Library" onClick={() => setSearching(true)} className="flex size-8 items-center justify-center rounded-full text-muted hover:bg-surface hover:text-text">
              <Icon name="search" size={16} />
            </button>
          )}
        </div>
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto" key={tab}>
        {error && <p className="m-0 px-8 pt-6 text-[13px] text-danger">{error}</p>}
        {items != null && list.length === 0 && (
          <p className="m-0 pt-[24vh] text-center text-[15px] text-muted">{tab === 'favourites' && !needle ? 'Circle a print with F to hang it here.' : needle ? 'Nothing matches.' : 'Your prints land here as they develop.'}</p>
        )}
        {list.length > 0 && (
          <>
            <CoverFlow items={list} index={focus} onIndex={setFocus} tall={tab === 'favourites'} justMarked={justMarked} paused={!!open} />
            {tab === 'all' && <Grid items={list} focusId={list[focus]?.id} onPick={(i) => setFocus(list.indexOf(i))} />}
          </>
        )}
      </div>

      {open && <Viewer item={open} list={list.some((i) => i.id === open.id) ? list : items!} all={items!} modelName={modelName} onFavourite={toggleFavourite} onDelete={remove} justMarked={justMarked} />}
    </div>
  );
}

// ---------- The stage: cover flow ----------

const SIDE = 5;

/**
 * One print faces you; the rest stand on either side, turned away, like prints flicked through in a
 * box. ←/→, the wheel or a click moves along; Enter or a click on the middle one opens it.
 */
function CoverFlow({ items, index, onIndex, tall, justMarked, paused }: { items: ImageItem[]; index: number; onIndex: (i: number) => void; tall: boolean; justMarked: string | null; paused: boolean }) {
  const stage = useRef<HTMLDivElement>(null);
  const [h, setH] = useState(0);
  const wheel = useRef(0);
  const go = (i: number) => onIndex(Math.max(0, Math.min(items.length - 1, i)));

  useEffect(() => {
    const el = stage.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setH(el.clientHeight));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  useEffect(() => {
    if (paused) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
      if (e.key === 'ArrowLeft') go(index - 1);
      else if (e.key === 'ArrowRight') go(index + 1);
      else if (e.key === 'Enter' && items[index]) view(items[index].id);
    };
    addEventListener('keydown', onKey);
    return () => removeEventListener('keydown', onKey);
  });

  const cur = items[index];
  const card = Math.max(160, h - 28);
  const ar = (i: ImageItem) => i.params.width / i.params.height;
  const centerW = cur ? card * Math.min(ar(cur), 1.6) : card;

  return (
    <section aria-label="Prints" aria-roledescription="carousel" className={cx('flex flex-col items-center', tall ? 'h-[calc(100%-8px)] min-h-[420px]' : 'h-[54vh] min-h-[320px]')}>
      <div
        ref={stage}
        className="coverflow relative w-full flex-1"
        onWheel={(e) => {
          const d = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.shiftKey ? e.deltaY : 0;
          if (!d) return;
          const now = Date.now();
          if (now - wheel.current < 140) return;
          wheel.current = now;
          go(index + Math.sign(d));
        }}
      >
        {items.map((it, i) => {
          const d = i - index;
          if (Math.abs(d) > SIDE) return null;
          const w = card * Math.min(ar(it), 1.6);
          const side = Math.sign(d);
          const x = d === 0 ? 0 : side * (centerW / 2 + card * 0.22 + (Math.abs(d) - 1) * card * 0.26);
          const style = {
            width: w,
            height: card,
            zIndex: 50 - Math.abs(d),
            opacity: Math.abs(d) === SIDE ? 0 : 1,
            transform: `translate(-50%, 0) translateX(${x}px) translateZ(${d === 0 ? 0 : -card * 0.55 - (Math.abs(d) - 1) * 40}px) rotateY(${-side * 58}deg)`,
          } as CSSProperties;
          return (
            <button
              key={it.id}
              type="button"
              aria-label={d === 0 ? `Open frame ${frameNo(it.frame)}: ${it.prompt}` : `Frame ${frameNo(it.frame)}`}
              aria-current={d === 0}
              tabIndex={d === 0 ? 0 : -1}
              onClick={() => (d === 0 ? view(it.id) : go(i))}
              className="cf-card absolute top-3 left-1/2 overflow-hidden rounded-[14px]"
              style={style}
            >
              <img src={api.imageUrl(it.id, Math.abs(d) <= 1 ? 1024 : 512)} alt="" draggable={false} className="size-full object-cover" />
              <span className="cf-shade" aria-hidden="true" />
            </button>
          );
        })}
      </div>
      {cur && (
        <div className="flex h-10 max-w-[min(640px,80%)] flex-none items-center gap-3">
          <p className="m-0 truncate text-[13.5px] text-text">{cur.prompt}</p>
          <span className="edge relative flex-none px-1.5 py-0.5 text-[11px] text-muted">
            {frameNo(cur.frame)}
            {cur.favourite && (
              <span className="pointer-events-none absolute -inset-x-2 -inset-y-2">
                <GreasePencil draw={justMarked === cur.id} />
              </span>
            )}
          </span>
        </div>
      )}
    </section>
  );
}

// ---------- The grid: every print, by sitting ----------

function Grid({ items, focusId, onPick }: { items: ImageItem[]; focusId: string | undefined; onPick: (i: ImageItem) => void }) {
  const sessions = useMemo(() => sessionsOf(items), [items]);
  return (
    <div className="flex flex-col gap-8 px-8 pt-6 pb-12">
      {sessions.map((s) => {
        const prints = s.threads.flatMap((t) => [...t.items].reverse());
        return (
          <section key={s.key} className="flex flex-col gap-3">
            <h2 className="m-0 text-[13px] font-medium text-muted">
              <span className="text-text">{dayLabel(s.start)}</span> · {clock(s.start)}
            </h2>
            <ul className="m-0 grid list-none grid-cols-[repeat(auto-fill,minmax(150px,1fr))] gap-2.5 p-0">
              {prints.map((i) => (
                <li key={i.id}>
                  <button
                    type="button"
                    onClick={() => view(i.id)}
                    onMouseEnter={() => onPick(i)}
                    onFocus={() => onPick(i)}
                    aria-label={`Frame ${frameNo(i.frame)}: ${i.prompt}`}
                    className={cx('tile block aspect-square w-full overflow-hidden rounded-[10px]', focusId === i.id && 'is-focus')}
                  >
                    <img src={api.imageUrl(i.id, 384)} alt="" loading="lazy" draggable={false} className="size-full object-cover" />
                  </button>
                </li>
              ))}
            </ul>
          </section>
        );
      })}
    </div>
  );
}

// ---------- The viewer: one print, full-bleed ----------

function Viewer({ item, list, all, modelName, onFavourite, onDelete, justMarked }: { item: ImageItem; list: ImageItem[]; all: ImageItem[]; modelName: (id: string) => string; onFavourite: (i: ImageItem) => void; onDelete: (i: ImageItem) => void; justMarked: string | null }) {
  const index = list.findIndex((i) => i.id === item.id);
  const prev = list[index - 1];
  const next = list[index + 1];
  const [details, setDetails] = useState(false);
  const [pathNote, setPathNote] = useState(false);
  // Which way we moved, so the next print slides in from that side, like going through a box of prints.
  const dir = useRef<'next' | 'prev' | null>(null);
  const lastIndex = useRef(index);
  if (lastIndex.current !== index) {
    dir.current = index > lastIndex.current ? 'next' : 'prev';
    lastIndex.current = index;
  }
  const strip = useRef<HTMLOListElement>(null);
  const p = item.params;
  const ms = jobMs(item);
  const chain = chainOf(item, all);
  const edits = editsOf(item, all);
  const parent = chain.length > 1 ? chain[chain.length - 2] : null;

  const go = (to: ImageItem | undefined) => to && view(to.id);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
      if (e.key === 'Escape') view(null);
      else if (e.key === 'ArrowLeft') go(prev);
      else if (e.key === 'ArrowRight') go(next);
      else if (e.key.toLowerCase() === 'f') onFavourite(item);
      else if (e.key.toLowerCase() === 'i') setDetails((d) => !d);
    };
    addEventListener('keydown', onKey);
    return () => removeEventListener('keydown', onKey);
  });
  // Keep the current frame in the middle of the contact strip.
  useEffect(() => {
    strip.current?.querySelector<HTMLElement>('[aria-current="true"]')?.scrollIntoView({ inline: 'center', block: 'nearest', behavior: document.documentElement.dataset.motion === 'off' ? 'auto' : 'smooth' });
  }, [item.id]);

  const openFolder = async () => {
    if (!item.output_path) return;
    if (await reveal(item.output_path)) return;
    await navigator.clipboard.writeText(item.output_path);
    setPathNote(true);
  };

  const rows: [string, string][] = [
    ['Model', modelName(item.model)],
    ['Seed', String(item.seed)],
    ['Steps', String(p.steps)],
    ['Guidance', String(p.cfg)],
    ['Size', `${p.width} × ${p.height}`],
    ['Time', ms != null ? duration(ms) : 'unknown'],
    ['Made', new Date(item.created_ms).toLocaleString(undefined, { day: 'numeric', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit' })],
  ];

  return (
    <div role="dialog" aria-modal="true" aria-label={`Frame ${frameNo(item.frame)}`} className="viewer enter absolute inset-0 z-30 flex">
      <div className="relative flex min-w-0 flex-1 flex-col">
        <div className="flex h-12 flex-none items-center justify-between gap-3 px-4">
          <Button variant="ghost" size="sm" icon="back" onClick={() => view(null)} className="text-[13px]">
            Library <span className="edge ml-1 text-muted">Esc</span>
          </Button>
          <span className="edge text-muted">
            {index + 1} of {list.length}
          </span>
          <div className="flex items-center gap-1.5">
            <Button size="sm" variant="ghost" icon={item.favourite ? 'check' : 'circle'} aria-pressed={item.favourite} onClick={() => onFavourite(item)} title="Favourite (F)">
              {item.favourite ? 'On the wall' : 'Favourite'}
            </Button>
            <ConfirmButton label="Delete" confirm="Delete for good?" onConfirm={() => onDelete(item)} />
            <Button size="sm" variant="ghost" aria-pressed={details} onClick={() => setDetails((d) => !d)} title="Show or hide the recipe (I)">
              {details ? 'Hide recipe' : 'Recipe'}
            </Button>
          </div>
        </div>

        <div className="relative flex min-h-0 flex-1 items-center justify-center px-16 pb-3">
          <figure className="m-0 flex h-full max-w-full flex-col items-center justify-center gap-2.5">
            <div className="relative flex min-h-0 max-w-full flex-1 items-center justify-center">
              <img key={item.id} src={api.imageUrl(item.id)} alt={item.prompt} draggable={false} className={cx('max-h-full max-w-full object-contain shadow-[var(--shadow-print)]', dir.current && `print-in-${dir.current}`)} />
              {item.favourite && justMarked === item.id && (
                <span className="pointer-events-none absolute inset-0 flex items-center justify-center">
                  <span className="relative" style={{ aspectRatio: `${p.width} / ${p.height}`, maxHeight: '100%', maxWidth: '100%', height: '100%' }}>
                    <GreasePencil draw={justMarked === item.id} />
                  </span>
                </span>
              )}
            </div>
            <figcaption className="edge selectable flex-none text-muted">
              ▸ {frameNo(item.frame)} ▸ seed {item.seed} ▸ {p.steps} steps{ms != null ? ` ▸ ${duration(ms)}` : ''} ▸ {p.width} × {p.height}
            </figcaption>
          </figure>
          <button type="button" aria-label="Previous print (←)" disabled={!prev} onClick={() => go(prev)} className="viewer-nav left-3">
            <Icon name="back" size={22} />
          </button>
          <button type="button" aria-label="Next print (→)" disabled={!next} onClick={() => go(next)} className="viewer-nav right-3">
            <Icon name="next" size={22} />
          </button>
        </div>

        {/* The contact strip: the whole view, the current frame in the middle. */}
        <div className="filmstrip flex-none overflow-x-auto">
          <ol ref={strip} className="m-0 flex w-max list-none gap-1.5 px-[50%] py-4">
            {list.map((t) => (
              <li key={t.id}>
                <button type="button" aria-label={`Frame ${frameNo(t.frame)}: ${t.prompt}`} aria-current={t.id === item.id} onClick={() => view(t.id)} className={cx('block h-[52px] overflow-hidden rounded-[2px] transition-opacity', t.id === item.id ? 'opacity-100 outline-2 outline-offset-2 outline-[#fcf8f5]' : 'opacity-50 hover:opacity-90')} style={{ aspectRatio: `${t.params.width} / ${t.params.height}` }}>
                  <img src={api.imageUrl(t.id, 256)} alt="" loading="lazy" draggable={false} className="size-full object-cover" />
                </button>
              </li>
            ))}
          </ol>
        </div>
      </div>

      {details && (
        <aside aria-label="The recipe" className="flex w-[340px] flex-none flex-col gap-5 overflow-y-auto border-l border-line bg-bg px-6 pt-5 pb-5">
          <div className="flex flex-col gap-2">
            <span className="eyebrow text-muted">{p.task === 'edit' ? 'Instruction' : 'Prompt'}</span>
            <p className="selectable m-0 text-[15px] leading-normal">{item.prompt}</p>
            {p.negative_prompt && <p className="selectable m-0 text-[12.5px] leading-snug text-muted">Without: {p.negative_prompt}</p>}
            {p.auto && <p className="m-0 text-[12px] leading-snug text-muted">Auto picked it: {p.auto}</p>}
          </div>

          {(chain.length > 1 || edits.length > 0 || (p.task === 'edit' && !parent)) && (
            <div className="flex flex-col gap-2 border-t border-line pt-4">
              <span className="eyebrow text-muted">Edited from</span>
              {chain.length > 1 ? (
                <ol className="m-0 flex list-none flex-wrap items-center gap-1.5 p-0">
                  {chain.map((c, n) => (
                    <li key={c.id} className="flex items-center gap-1.5">
                      {n > 0 && <span className="text-muted">→</span>}
                      <Thumb item={c} current={c.id === item.id} />
                    </li>
                  ))}
                </ol>
              ) : (
                <p className="m-0 text-[12.5px] leading-snug text-muted">{p.task === 'edit' ? 'An image from outside the Library.' : 'Not an edit.'}</p>
              )}
              {edits.length > 0 && (
                <>
                  <span className="eyebrow pt-2 text-muted">Edits made from it</span>
                  <div className="flex flex-wrap gap-1.5">
                    {edits.map((c) => (
                      <Thumb key={c.id} item={c} current={false} />
                    ))}
                  </div>
                </>
              )}
            </div>
          )}

          <dl className="selectable m-0 grid grid-cols-[84px_1fr] gap-y-[9px] border-t border-line pt-4 font-mono text-[12px]">
            {rows.map(([k, v]) => (
              <div key={k} className="contents">
                <dt className="text-muted">{k}</dt>
                <dd className="m-0">{v}</dd>
              </div>
            ))}
          </dl>

          <div className="flex-1" />
          <div className="flex flex-col gap-2">
            <Button variant="solid" icon="edit" onClick={() => editThis(item)} className="h-10 rounded-card text-[14px]">
              Edit this
            </Button>
            <div className="grid grid-cols-2 gap-2">
              <Button icon="vary" onClick={() => varyJob(item, parent)} title="Same recipe, new seed: develops on Create">
                Vary
              </Button>
              <Button icon="create" onClick={() => remix(item)} title="Load the whole recipe into Create">
                Open in Create
              </Button>
              <CopyButton text={recipeText(item)} size="md">
                Copy recipe
              </CopyButton>
              <Button icon="folder" onClick={() => void openFolder()}>
                Open folder
              </Button>
            </div>
            {pathNote && <p className="selectable m-0 text-[11.5px] leading-snug text-muted">Copied the file's path. Open folder works in the desktop app; this is the browser preview.</p>}
          </div>
        </aside>
      )}
    </div>
  );
}

function Thumb({ item, current }: { item: ImageItem; current: boolean }) {
  return (
    <button type="button" onClick={() => view(item.id)} aria-label={`Frame ${frameNo(item.frame)}`} aria-current={current} title={`${frameNo(item.frame)} · ${item.prompt}`} className={cx('flex flex-col items-center gap-0.5', current ? 'text-text' : 'text-muted hover:text-text')}>
      <img src={api.imageUrl(item.id, 256)} alt="" draggable={false} className={cx('size-12 rounded-[2px] object-cover', current ? 'outline-[1.5px] outline-offset-1 outline-text' : 'opacity-80')} />
      <span className="edge text-[10px]">{frameNo(item.frame)}</span>
    </button>
  );
}
