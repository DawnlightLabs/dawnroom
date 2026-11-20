// Settings: the darkroom's control wall. The left side is a panel of instruments, each showing its live
// reading (the theme, the disk, the GPU); the right side is one section at a time, each with something
// to see rather than a form to read: the two rooms in miniature, your own print developing, the disk
// as a map, the GPU on a timer dial, keys you could press.

import { useEffect, useState, type CSSProperties, type ReactNode } from 'react';
import { api, loadInfo, setMotion, setTheme, useApp, type Theme } from '../app/state';
import type { ImageItem } from '../api/types';
import { Grain } from '../components/Moments';
import { StorageSection } from '../components/Storage';
import { cx, CopyButton, HowLink, Icon, Lockup, Mark, Orb, StatusDot, type IconName } from '../components/ui';
import { DOCS, ENGINE_URL, LICENCE, PRODUCT_NAME, STUDIO_VERSION } from '../config';
import { DAWNLIGHT_URL } from '../lib/brand';
import { bytes, shortGpu, vram } from '../lib/format';
import { inTauri, openExternal, reveal } from '../lib/tauri';
import { checkForUpdate, restartBlocker, restartToUpdate, setSignIn, setUpdates, trayLines, useBg } from '../lib/background';

type SectionId = 'general' | 'appearance' | 'motion' | 'storage' | 'engine' | 'background' | 'agents' | 'about';

const SECTIONS: { id: SectionId; label: string; icon: IconName; blurb: string; docs?: string }[] = [
  { id: 'appearance', label: 'Appearance', icon: 'image', blurb: 'Two rooms. Ink for working at night, paper for the daylight. Or let Windows decide.' },
  { id: 'motion', label: 'Motion', icon: 'vary', blurb: 'The grain, the safelight and the print rising out of the developer. All of it can stand still.' },
  { id: 'storage', label: 'Storage', icon: 'folder', blurb: 'Everything Dawnroom keeps on this drive, and how to get space back.', docs: DOCS.storage },
  { id: 'engine', label: 'Engine and GPU', icon: 'models', blurb: 'The engine that runs the models here, and the card it runs them on.', docs: DOCS.offload },
  { id: 'background', label: 'Tray and updates', icon: 'tray', blurb: `${PRODUCT_NAME} can keep running in the tray, start when you sign in, and update itself.`, docs: DOCS.tray },
  { id: 'general', label: 'Keys and folders', icon: 'settings', blurb: 'Where your work lives, and the keys that save a click.' },
  { id: 'agents', label: 'Connect an agent', icon: 'open', blurb: `Let Claude Code or Codex make and edit images through ${PRODUCT_NAME}. Everything still runs on this machine.`, docs: DOCS.agents },
  { id: 'about', label: 'About', icon: 'circle', blurb: 'Open source, local by default, no credits.' },
];

const KEY = 'studio.settings.section';

export function Settings() {
  const [section, setSection] = useState<SectionId>(() => {
    try {
      const v = sessionStorage.getItem(KEY) as SectionId | null;
      return v && SECTIONS.some((s) => s.id === v) ? v : 'appearance';
    } catch {
      return 'appearance';
    }
  });
  useEffect(() => {
    try {
      sessionStorage.setItem(KEY, section);
    } catch {
      // private mode
    }
  }, [section]);
  // Folder sizes are walked on request, so ask when the page opens rather than on every poll.
  useEffect(() => {
    void loadInfo();
  }, []);
  const s = SECTIONS.find((x) => x.id === section) ?? SECTIONS[0];

  return (
    <div className="flex min-h-0 min-w-0 flex-1">
      <nav aria-label="Settings" className="flex w-[264px] flex-none flex-col border-r border-line pt-6 pb-4">
        <h1 className="m-0 px-6 pb-5 font-display text-[30px] leading-none font-light tracking-[-0.02em]">Settings</h1>
        <div className="flex flex-col">
          {SECTIONS.map((x) => (
            <button
              key={x.id}
              type="button"
              aria-current={x.id === s.id ? 'page' : undefined}
              onClick={() => setSection(x.id)}
              className={cx('group relative flex h-12 items-center gap-3 px-6 text-left text-[13.5px] transition-colors', x.id === s.id ? 'bg-surface text-text' : 'text-muted hover:bg-surface/60 hover:text-text')}
            >
              {/* The lit instrument has its lamp on: a terracotta → sun line at the edge. */}
              <span className={cx('absolute inset-y-2 left-0 w-[3px] rounded-r-full bg-safelight transition-opacity', x.id === s.id ? 'opacity-100' : 'opacity-0')} aria-hidden="true" />
              <Icon name={x.icon} size={15} />
              <span className="flex-1">{x.label}</span>
              <Reading id={x.id} />
            </button>
          ))}
        </div>
      </nav>

      <main key={s.id} className="enter flex min-w-0 flex-1 flex-col overflow-y-auto">
        <header className="flex flex-col gap-2 px-12 pt-10 pb-7">
          <h2 className="m-0 font-display text-[44px] leading-[1.05] font-light tracking-[-0.025em]">{s.label}</h2>
          <p className="m-0 max-w-[60ch] text-[14px] leading-snug text-muted">
            {s.blurb}
            {s.docs && (
              <>
                {' '}
                <HowLink href={s.docs} />
              </>
            )}
          </p>
        </header>
        <div className="flex max-w-[980px] flex-col gap-8 px-12 pb-14">
          {s.id === 'appearance' && <Appearance />}
          {s.id === 'motion' && <Motion />}
          {s.id === 'storage' && <StorageSection />}
          {s.id === 'engine' && <Engine />}
          {s.id === 'background' && <Background />}
          {s.id === 'general' && <General />}
          {s.id === 'agents' && <Agents />}
          {s.id === 'about' && <About />}
        </div>
      </main>
    </div>
  );
}

/** Each instrument's live reading, at the right of its row. */
function Reading({ id }: { id: SectionId }) {
  const theme = useApp((s) => s.theme);
  const motion = useApp((s) => s.motion);
  const disk = useApp((s) => s.disk);
  const gpu = useApp((s) => s.health?.gpu ?? s.gpu);
  const busy = useApp((s) => !!s.health?.busy);
  const signIn = useBg((s) => s.signIn);
  const update = useBg((s) => s.ready);
  const cls = 'edge text-[11px] text-muted';
  if (id === 'appearance') return <span className={cls}>{theme === 'system' ? 'system' : theme}</span>;
  if (id === 'motion') return <span className={cls}>{motion === 'reduced' ? 'still' : 'on'}</span>;
  if (id === 'storage' && disk) return <span className={cls}>{bytes(disk.free_bytes)} free</span>;
  if (id === 'engine' && gpu)
    return (
      <span className="flex items-center gap-1.5">
        <span className="h-[4px] w-7 overflow-hidden rounded-full bg-line-strong">
          <span className={cx('block h-full', busy ? 'bg-safelight' : 'bg-text/60')} style={{ width: `${Math.max(4, (gpu.vram_used_mib / gpu.vram_total_mib) * 100)}%` }} />
        </span>
        <span className={cls}>{(gpu.vram_used_mib / 1024).toFixed(1)}</span>
      </span>
    );
  if (id === 'background') return update || signIn ? <span className={cls}>{update ? `${update} ready` : 'at sign-in'}</span> : null;
  if (id === 'about') return <span className={cls}>{STUDIO_VERSION}</span>;
  return null;
}

// ---------- Appearance: the two rooms ----------

const ROOMS = {
  dark: { bg: '#191B1D', surface: '#202326', line: 'rgb(252 248 245 / .12)', text: '#fcf8f5', muted: '#a6a9ae' },
  light: { bg: '#fcf8f5', surface: '#f5efe8', line: 'rgb(25 27 29 / .14)', text: '#191B1D', muted: '#5e5a54' },
};

/** Studio in miniature, painted in one room's own colours. */
function Room({ t }: { t: 'dark' | 'light' }) {
  const c = ROOMS[t];
  return (
    <div className="absolute inset-0 flex" style={{ background: c.bg }} aria-hidden="true">
      <div className="flex w-[9%] flex-col items-center gap-[7%] pt-[6%]" style={{ borderRight: `1px solid ${c.line}` }}>
        {[0, 1, 2, 3].map((i) => (
          <span key={i} className="aspect-square w-[42%] rounded-[3px]" style={{ background: i === 0 ? c.surface : 'transparent', border: `1px solid ${c.line}` }} />
        ))}
      </div>
      <div className="flex flex-1 flex-col gap-[5%] p-[6%]">
        <span className="h-[4%] w-[46%] rounded-full" style={{ background: c.text, opacity: 0.85 }} />
        <span className="h-[3%] w-[30%] rounded-full" style={{ background: c.muted, opacity: 0.7 }} />
        <span className="relative aspect-[4/3] w-[54%] overflow-hidden rounded-[4px]" style={{ background: 'linear-gradient(160deg, #2c4a6b 0%, #c47855 62%, #ffd12b 100%)', boxShadow: t === 'dark' ? '0 10px 24px rgb(0 0 0 / .5)' : '0 8px 18px rgb(25 27 29 / .16)' }}>
          <span className="absolute right-[12%] bottom-[18%] aspect-square w-[22%] rounded-full" style={{ background: '#ffe347', opacity: 0.9 }} />
        </span>
        <span className="mt-auto flex h-[16%] items-center gap-[4%] rounded-[6px] px-[4%]" style={{ background: c.surface, border: `1px solid ${c.line}` }}>
          <span className="h-[18%] w-[38%] rounded-full" style={{ background: c.muted, opacity: 0.6 }} />
          <span className="ml-auto h-[52%] w-[18%] rounded-[4px] bg-safelight" />
        </span>
      </div>
    </div>
  );
}

function Appearance() {
  const theme = useApp((s) => s.theme);
  const osDark = typeof matchMedia !== 'undefined' && matchMedia('(prefers-color-scheme: dark)').matches;
  const options: { id: Theme; label: string; line: string }[] = [
    { id: 'dark', label: 'Ink', line: 'The darkroom. The print is the brightest thing on screen.' },
    { id: 'light', label: 'Paper', line: 'The daylight room. Prints rise out of white, like in the tray.' },
    { id: 'system', label: 'System', line: `Follows Windows, now ${osDark ? 'ink' : 'paper'}.` },
  ];
  return (
    <div role="radiogroup" aria-label="Theme" className="grid grid-cols-3 gap-5">
      {options.map((o) => {
        const on = theme === o.id;
        return (
          <button key={o.id} type="button" role="radio" aria-checked={on} onClick={() => setTheme(o.id)} className="group flex flex-col gap-3 text-left">
            <span className={cx('relative block aspect-[4/3] overflow-hidden rounded-[12px] transition-[box-shadow,transform] duration-200', on ? 'shadow-[0_0_0_2px_var(--color-bg),0_0_0_4px_var(--color-text),var(--shadow-print)]' : 'shadow-[0_0_0_1px_var(--color-line-strong)] group-hover:-translate-y-0.5')}>
              {o.id === 'system' ? (
                <>
                  <Room t="light" />
                  <span className="absolute inset-0 [clip-path:polygon(0_0,62%_0,38%_100%,0_100%)]">
                    <Room t="dark" />
                  </span>
                </>
              ) : (
                <Room t={o.id} />
              )}
            </span>
            <span className="flex items-center gap-2.5">
              <span className={cx('flex size-[14px] flex-none items-center justify-center rounded-full border-[1.5px]', on ? 'border-text' : 'border-muted')}>{on && <span className="size-[6px] rounded-full bg-text" />}</span>
              <span className="text-[15px] font-medium">{o.label}</span>
            </span>
            <span className="-mt-1.5 pl-[24px] text-[12.5px] leading-snug text-muted">{o.line}</span>
          </button>
        );
      })}
    </div>
  );
}

// ---------- Motion: your own print, developing ----------

/** The latest print in the Library, for the motion specimen. */
function useLatestPrint(): ImageItem | null {
  const [item, setItem] = useState<ImageItem | null>(null);
  useEffect(() => {
    api.images({ limit: 1 }).then((p) => setItem(p.items[0] ?? null), () => {});
  }, []);
  return item;
}

/** The developing ritual on a loop: 0 → 1 over three seconds, a breath, again. Still when motion is off. */
function useDevelopLoop(on: boolean): number {
  const [p, setP] = useState(1);
  useEffect(() => {
    if (!on) return setP(1);
    let t = 0;
    const id = setInterval(() => {
      t = (t + 1) % 18;
      setP(Math.min(1, t / 12));
    }, 250);
    return () => clearInterval(id);
  }, [on]);
  return p;
}

function Motion() {
  const motion = useApp((s) => s.motion);
  const reducedOs = typeof matchMedia !== 'undefined' && matchMedia('(prefers-reduced-motion: reduce)').matches;
  const still = motion === 'reduced' || reducedOs;
  const print = useLatestPrint();
  const p = useDevelopLoop(!still);
  const options = [
    { id: 'system' as const, label: 'Moving', line: reducedOs ? 'Windows asks for less animation, so this stays still.' : 'Follows your Windows animation setting.' },
    { id: 'reduced' as const, label: 'Still', line: 'Plain previews, no grain or glow. Nothing moves.' },
  ];
  return (
    <div className="grid grid-cols-[minmax(0,1.25fr)_minmax(0,1fr)] items-start gap-8">
      <figure className="m-0 flex flex-col gap-2.5">
        <div className="developing relative aspect-[4/3] overflow-hidden rounded-[12px] shadow-[0_0_0_1px_var(--color-line),var(--shadow-print)]" style={{ '--p': p } as CSSProperties}>
          {print ? <img src={api.imageUrl(print.id, 768)} alt="" draggable={false} className="preview" /> : <div className="absolute inset-0 bg-surface" />}
          <div className="tint" />
          {!still && <Grain />}
          <div className="absolute inset-x-0 bottom-0 h-[3px] bg-text/10">
            <div className="h-full bg-safelight transition-[width] duration-200" style={{ width: `${p * 100}%` }} />
          </div>
        </div>
        <figcaption className="edge text-muted">{still ? 'Still: the plain preview, as it arrives.' : print ? 'Your last print, developing.' : 'Developing.'}</figcaption>
      </figure>
      <div role="radiogroup" aria-label="Motion" className="flex flex-col gap-3">
        {options.map((o) => {
          const on = motion === o.id;
          return (
            <button key={o.id} type="button" role="radio" aria-checked={on} onClick={() => setMotion(o.id)} className={cx('flex items-start gap-3 rounded-[12px] border px-4 py-3.5 text-left transition-colors', on ? 'border-text/50 bg-surface' : 'border-line hover:border-line-strong')}>
              <span className={cx('mt-0.5 flex size-[14px] flex-none items-center justify-center rounded-full border-[1.5px]', on ? 'border-text' : 'border-muted')}>{on && <span className="size-[6px] rounded-full bg-text" />}</span>
              <span className="flex flex-col gap-0.5">
                <span className="text-[14.5px] font-medium">{o.label}</span>
                <span className="text-[12.5px] leading-snug text-muted">{o.line}</span>
              </span>
            </button>
          );
        })}
        <p className="m-0 pt-1 text-[12.5px] leading-snug text-muted">Also under Motion: the still safelight glow while a job runs, the grease-pencil circle on favourites, and the opening while the engine starts.</p>
      </div>
    </div>
  );
}

// ---------- Engine and GPU: the timer dial ----------

/** A darkroom timer's dial: 270° of arc, ticks every 1 GB, the hand at what's in use. */
function Dial({ used, total, busy }: { used: number; total: number; busy: boolean }) {
  const r = 92;
  const cx0 = 120;
  const cy0 = 120;
  // 0 GB at the bottom left, full at the bottom right, clockwise like a timer.
  const start = 225;
  const sweep = 270;
  const share = Math.min(1, used / Math.max(1, total));
  const point = (deg: number, rr = r) => {
    const a = ((deg - 90) * Math.PI) / 180;
    return [cx0 + rr * Math.cos(a), cy0 + rr * Math.sin(a)];
  };
  const arc = (from: number, to: number) => {
    const [x1, y1] = point(from);
    const [x2, y2] = point(to);
    return `M${x1} ${y1} A${r} ${r} 0 ${to - from > 180 ? 1 : 0} 1 ${x2} ${y2}`;
  };
  const gb = Math.round(total / 1024);
  return (
    <svg viewBox="0 0 240 220" className="w-full max-w-[300px]" role="img" aria-label={`${(used / 1024).toFixed(1)} of ${gb} GB of graphics memory in use`}>
      <defs>
        <linearGradient id="dial-safelight" x1="0" y1="1" x2="1" y2="0">
          <stop offset="0" stopColor="#c47855" />
          <stop offset="1" stopColor="#ffd12b" />
        </linearGradient>
      </defs>
      <path d={arc(start, start + sweep)} fill="none" stroke="var(--color-line-strong)" strokeWidth="10" strokeLinecap="round" />
      {share > 0.005 && <path d={arc(start, start + sweep * share)} fill="none" stroke={busy ? 'url(#dial-safelight)' : 'var(--color-text)'} strokeOpacity={busy ? 1 : 0.8} strokeWidth="10" strokeLinecap="round" className="transition-all duration-500" />}
      {Array.from({ length: gb + 1 }, (_, i) => {
        const deg = start + (sweep * i) / gb;
        const [x1, y1] = point(deg, r - 16);
        const [x2, y2] = point(deg, r - (i % 2 === 0 ? 26 : 21));
        const [lx, ly] = point(deg, r - 38);
        return (
          <g key={i}>
            <line x1={x1} y1={y1} x2={x2} y2={y2} stroke="var(--color-muted)" strokeWidth="1.5" />
            {i % 2 === 0 && (
              <text x={lx} y={ly + 3.5} textAnchor="middle" fontSize="10" fill="var(--color-muted)" fontFamily="var(--font-mono)">
                {i}
              </text>
            )}
          </g>
        );
      })}
      <text x={cx0} y={cy0 + 6} textAnchor="middle" fontSize="34" fill="var(--color-text)" fontFamily="var(--font-mono)">
        {(used / 1024).toFixed(1)}
      </text>
      <text x={cx0} y={cy0 + 26} textAnchor="middle" fontSize="11" fill="var(--color-muted)" fontFamily="var(--font-mono)">
        of {gb} GB in use
      </text>
    </svg>
  );
}

function Engine() {
  const health = useApp((s) => s.health);
  const gpu = useApp((s) => s.health?.gpu ?? s.gpu);
  const hardware = useApp((s) => s.hardware);
  const models = useApp((s) => s.models);
  const loaded = models.find((m) => m.id === health?.model)?.name;
  const choice = hardware?.choice;
  const facts: [string, ReactNode][] = [
    ['Runtime', `${choice?.build ?? health?.runtime ?? '…'}${choice?.experimental ? ' · experimental' : ''}`],
    ['System memory', hardware ? `${Math.round(hardware.ram.total_bytes / 2 ** 30)} GB` : '…'],
    ...(hardware?.gpus ?? []).map((g): [string, ReactNode] => [g.integrated ? 'Built-in graphics' : 'Graphics card', `${g.name}${g.integrated ? '' : ` · ${vram(g.vram_mib)}${g.driver ? ` · driver ${g.driver}` : ''}`}`]),
    ['Engine', `${health?.version ?? '…'} at ${ENGINE_URL.replace('http://', '')}, this computer only`],
  ];
  return (
    <>
      <div className="grid grid-cols-[300px_minmax(0,1fr)] items-center gap-10 rounded-[16px] border border-line bg-surface px-8 py-7">
        {gpu ? <Dial used={gpu.vram_used_mib} total={gpu.vram_total_mib} busy={!!health?.busy} /> : <p className="m-0 text-muted">No GPU found.</p>}
        <div className="flex flex-col gap-3">
          <span className="eyebrow text-muted">On the bench</span>
          <span className="font-display text-[30px] leading-tight font-light tracking-[-0.02em]">{gpu ? shortGpu(gpu.name) : 'No GPU'}</span>
          <span className="flex items-center gap-2 text-[14px]">
            <StatusDot state={health?.busy ? 'busy' : loaded ? 'ok' : 'off'} />
            {health?.busy ? `Developing with ${loaded ?? 'a model'}` : loaded ? `${loaded} is loaded` : 'Nothing loaded'}
          </span>
          <span className="max-w-[44ch] text-[12.5px] leading-snug text-muted">The dial counts every app on this card. A model unloads after a few idle minutes, so other apps get the memory back; the next image then takes a little longer to start.</span>
        </div>
      </div>
      {choice?.message && (
        <p className="m-0 flex items-start gap-2 rounded-[12px] border border-line px-4 py-3 text-[13px] leading-snug">
          <Icon name="warn" size={15} />
          <span>
            {choice.message}{' '}
            {choice.link && (
              <button type="button" onClick={() => openExternal(choice.link!)} className="text-link underline underline-offset-2">
                Get the driver
              </button>
            )}
          </span>
        </p>
      )}
      <dl className="selectable m-0 grid grid-cols-[150px_minmax(0,1fr)] gap-x-6 gap-y-3 font-mono text-[12.5px]">
        {facts.map(([k, v]) => (
          <div key={k + String(v)} className="contents">
            <dt className="text-muted">{k}</dt>
            <dd className="m-0">{v}</dd>
          </div>
        ))}
      </dl>
    </>
  );
}

// ---------- Keys and folders ----------

/** A keycap you could press. */
const Key = ({ children, wide }: { children: ReactNode; wide?: boolean }) => (
  <kbd className={cx('inline-flex h-10 items-center justify-center rounded-[8px] border border-line-strong bg-surface px-3 font-mono text-[13px] text-text shadow-[0_3px_0_var(--color-line-strong)]', wide ? 'min-w-[72px]' : 'min-w-10')}>{children}</kbd>
);

function General() {
  const info = useApp((s) => s.info);
  const keys: { keys: ReactNode; what: string; where: string }[] = [
    { keys: (<><Key wide>Ctrl</Key><span className="text-muted">+</span><Key wide>Enter</Key></>), what: 'Develop', where: 'In the prompt box' },
    { keys: (<><Key wide>Ctrl</Key><span className="text-muted">+</span><Key>V</Key></>), what: 'Attach an image to edit', where: 'In the prompt box, or drop it anywhere on Create' },
    { keys: (<><Key>←</Key><Key>→</Key></>), what: 'Walk through your prints', where: 'Library and the viewer' },
    { keys: <Key>F</Key>, what: 'Favourite', where: 'In the viewer' },
    { keys: <Key>I</Key>, what: 'Show the recipe', where: 'In the viewer' },
    { keys: <Key wide>Esc</Key>, what: 'Close a panel or viewer', where: 'Anywhere' },
  ];
  return (
    <>
      <div className="grid grid-cols-2 gap-x-8 gap-y-6">
        {keys.map((k) => (
          <div key={k.what} className="flex items-center gap-4">
            <span className="flex w-[180px] flex-none items-center gap-1.5">{k.keys}</span>
            <span className="flex flex-col">
              <span className="text-[14px]">{k.what}</span>
              <span className="text-[12px] text-muted">{k.where}</span>
            </span>
          </div>
        ))}
      </div>
      {/* The data folder, labelled like a film canister. */}
      <div className="flex items-center gap-5 rounded-[16px] border border-line bg-surface px-6 py-5">
        <span className="flex h-14 w-10 flex-none flex-col overflow-hidden rounded-[6px] border border-line-strong" aria-hidden="true">
          <span className="h-2 bg-text/70" />
          <span className="flex-1 bg-safelight" />
          <span className="h-2 bg-text/70" />
        </span>
        <span className="flex min-w-0 flex-1 flex-col gap-1">
          <span className="eyebrow text-muted">Your work lives in</span>
          <span className="selectable font-mono text-[13px] break-all">{info?.data_dir ?? 'Reading…'}</span>
          <span className="text-[12px] text-muted">Every print is a PNG with its recipe inside. Move it all to another drive in Storage.</span>
        </span>
        {inTauri && info && (
          <button type="button" onClick={() => void reveal(info.data_dir)} className="flex h-9 flex-none items-center gap-1.5 rounded-control border border-line-strong px-3 text-[13px] hover:bg-raised">
            <Icon name="folder" size={14} />
            Open
          </button>
        )}
      </div>
    </>
  );
}

// ---------- Tray and updates: the taskbar corner ----------

/** A switch row: what it does on the left, the switch on the right. */
function Switch({ label, line, docs, on, disabled, onChange }: { label: string; line: ReactNode; docs?: string; on: boolean; disabled?: boolean; onChange: (on: boolean) => void }) {
  return (
    <label className={cx('flex items-start justify-between gap-6', disabled ? 'cursor-default opacity-60' : 'cursor-pointer')}>
      <span className="flex flex-col gap-1">
        <span className="text-[14px]">{label}</span>
        <span className="max-w-[62ch] text-[12.5px] leading-snug text-muted">
          {line}
          {docs && (
            <>
              {' '}
              <HowLink href={docs} />
            </>
          )}
        </span>
      </span>
      <input type="checkbox" role="switch" checked={on} disabled={disabled} onChange={(e) => onChange(e.target.checked)} className="peer sr-only" />
      <span aria-hidden="true" className={cx('relative mt-0.5 h-[18px] w-8 flex-none rounded-full transition-colors peer-focus-visible:outline-2 peer-focus-visible:outline-ring', on ? 'bg-text' : 'bg-line-strong')}>
        <span className={cx('absolute top-[2px] size-[14px] rounded-full bg-bg transition-[left]', on ? 'left-[16px]' : 'left-[2px]')} />
      </span>
    </label>
  );
}

const ago = (ms: number) => {
  const m = Math.round((Date.now() - ms) / 60000);
  return m < 1 ? 'just now' : m < 60 ? `${m} min ago` : m < 48 * 60 ? `${Math.round(m / 60)} h ago` : `${Math.round(m / 1440)} days ago`;
};

function Background() {
  const models = useApp((s) => s.models);
  const pulls = useApp((s) => s.pulls);
  const health = useApp((s) => s.health);
  const conn = useApp((s) => s.conn);
  const lines = trayLines({ models, pulls, health, conn });
  const b = useBg((s) => s);
  const wait = useApp(restartBlocker);
  const status = !inTauri
    ? 'In the desktop app.'
    : b.checking
      ? 'Checking…'
      : b.ready
        ? `${PRODUCT_NAME} ${b.ready} is downloaded and ready.`
        : !b.updates
          ? 'Off.'
          : b.lastCheck
            ? `You have the latest (${STUDIO_VERSION}), checked ${ago(b.lastCheck)}.`
            : 'Not checked yet.';
  const menu = [...lines.downloads, ...(lines.gpu ? [lines.gpu] : [])];
  return (
    <>
      {/* The taskbar corner with the tray menu open, showing what it shows right now. */}
      <div className="relative flex h-[232px] items-end overflow-hidden rounded-[16px] border border-line bg-surface" aria-hidden="true">
        <div className="absolute right-[64px] bottom-[52px] flex w-[260px] flex-col rounded-[8px] border border-line-strong bg-raised py-1.5 text-[12.5px] shadow-[0_12px_32px_rgb(0_0_0/0.25)]">
          <span className="px-3 py-1.5 text-text">Open {PRODUCT_NAME}</span>
          {menu.length > 0 && <span className="my-1 h-px bg-line" />}
          {menu.map((l) => (
            <span key={l} className="truncate px-3 py-1.5 text-muted">
              {l}
            </span>
          ))}
          <span className="my-1 h-px bg-line" />
          <span className="px-3 py-1.5 text-text">Quit {PRODUCT_NAME}</span>
        </div>
        <div className="flex h-11 w-full items-center justify-end gap-4 border-t border-line bg-bg px-5">
          <span className="flex size-7 items-center justify-center rounded-[6px] bg-raised">
            <Mark size={16} />
          </span>
          <span className="font-mono text-[11px] text-muted">{new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</span>
        </div>
      </div>
      <p className="m-0 -mt-4 text-[12.5px] leading-snug text-muted">Closing the window keeps {PRODUCT_NAME} in the tray, so downloads and agents carry on. Quit from its menu stops the engine and frees the GPU.</p>

      <div className="flex flex-col gap-6 border-t border-line pt-6">
        <Switch
          label="Start when you sign in"
          line={inTauri ? `${PRODUCT_NAME} opens in the tray, not as a window, so agents can reach it right away.` : 'Only in the desktop app.'}
          on={!!b.signIn}
          disabled={b.signIn === null}
          onChange={(on) => void setSignIn(on)}
        />
        <Switch
          label="Update automatically"
          line={`Once a day Studio asks the latest release on GitHub whether there's a newer ${PRODUCT_NAME}, and downloads it in the background. It's the only network call ${PRODUCT_NAME} makes on its own, and nothing about you or your images is sent. The update installs when you restart, or the next time you quit; never while an image develops or a model downloads.`}
          docs={DOCS.updates}
          on={b.updates}
          disabled={!inTauri}
          onChange={setUpdates}
        />
        <div className="-mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-[12.5px]">
          <span className={cx('edge', b.ready ? 'text-text' : 'text-muted')}>{status}</span>
          {inTauri && b.updates && b.checkFailed && !b.checking && <span className="edge text-muted">The last check couldn't reach GitHub.</span>}
          {b.ready &&
            (wait ? (
              <span className="edge text-muted">Restart {wait}.</span>
            ) : (
              <button type="button" disabled={b.installing} onClick={() => void restartToUpdate()} className="rounded-[4px] text-link underline underline-offset-2">
                {b.installing ? 'Restarting…' : 'Restart to update'}
              </button>
            ))}
          {inTauri && b.updates && !b.checking && !b.ready && (
            <button type="button" onClick={() => void checkForUpdate(true)} className="rounded-[4px] text-muted underline underline-offset-2 hover:text-text">
              Check now
            </button>
          )}
        </div>
      </div>
    </>
  );
}

// ---------- Connect an agent: a terminal ----------

function Agents() {
  const info = useApp((s) => s.info);
  const [agent, setAgent] = useState<'claude' | 'codex'>('claude');
  const exe = info?.exe ?? 'dawnroom';
  const commands = {
    claude: `claude mcp add --transport stdio dawnroom -- "${exe}" mcp`,
    // A TOML literal string (single quotes) keeps Windows backslashes as they are.
    codex: `[mcp_servers.dawnroom]\ncommand = '${exe}'\nargs = ["mcp"]`,
  };
  return (
    <>
      <div className="overflow-hidden rounded-[14px] bg-[#191B1D] text-[#fcf8f5] shadow-[0_0_0_1px_var(--color-line-strong),var(--shadow-print)]">
        <div className="flex items-center gap-4 border-b border-white/10 px-4">
          <span className="flex gap-1.5" aria-hidden="true">
            <span className="size-2.5 rounded-full bg-white/15" />
            <span className="size-2.5 rounded-full bg-white/15" />
            <span className="size-2.5 rounded-full bg-white/15" />
          </span>
          <div role="tablist" aria-label="Agent" className="flex">
            {(
              [
                ['claude', 'Claude Code'],
                ['codex', 'Codex'],
              ] as const
            ).map(([id, label]) => (
              <button key={id} type="button" role="tab" aria-selected={agent === id} onClick={() => setAgent(id)} className={cx('-mb-px border-b-[1.5px] px-3 py-2.5 font-mono text-[12px] transition-colors', agent === id ? 'border-[#ffd12b] text-[#fcf8f5]' : 'border-transparent text-[#a6a9ae] hover:text-[#fcf8f5]')}>
                {label}
              </button>
            ))}
          </div>
          <span className="ml-auto">
            <CopyButton text={commands[agent]} className="border-white/15 text-[#fcf8f5] hover:bg-white/10" />
          </span>
        </div>
        <pre tabIndex={0} aria-label={agent === 'claude' ? 'Command to add the MCP server to Claude Code' : 'Lines to add to Codex config'} className="selectable m-0 px-5 py-5 font-mono text-[13px] leading-relaxed break-all whitespace-pre-wrap">
          {agent === 'claude' ? <span className="text-[#c47855]">$ </span> : null}
          {commands[agent]}
        </pre>
      </div>
      <div className="grid grid-cols-3 gap-5">
        {[
          ['1', agent === 'claude' ? 'Run it in a terminal' : 'Add it to ~/.codex/config.toml', 'Once. The agent finds the engine by itself.'],
          ['2', 'Ask for an image', '"Make a poster for…", "edit this photo so…" The agent picks Auto unless you name a model.'],
          ['3', 'It never downloads alone', 'If a model is missing, the agent asks you first.'],
        ].map(([n, t, d]) => (
          <div key={n} className="flex flex-col gap-1.5">
            <span className="font-mono text-[12px] text-muted">{n}</span>
            <span className="text-[14px] font-medium">{t}</span>
            <span className="text-[12.5px] leading-snug text-muted">{d}</span>
          </div>
        ))}
      </div>
    </>
  );
}

// ---------- About: the dawn banner ----------

function About() {
  const health = useApp((s) => s.health);
  return (
    <>
      <div className="relative overflow-hidden rounded-[18px] px-10 pt-14 pb-10" style={{ background: 'linear-gradient(180deg, #191B1D 0%, #25272a 55%, #6b3a26 82%, #c47855 96%, #ffd12b 100%)' }}>
        <div className="relative flex flex-col gap-5 text-[#fcf8f5]">
          <Lockup cap={22} />
          <p className="m-0 max-w-[46ch] text-[15px] leading-snug text-[#fcf8f5]/85">An image studio that runs on your own GPU. Open source, local by default, no credits.</p>
          <span className="inline-flex items-center gap-1.5 text-[13px] text-[#fcf8f5]/80">
            by
            <button type="button" onClick={() => openExternal(DAWNLIGHT_URL)} className="inline-flex items-center gap-1.5 rounded-[4px] text-[#a8daf9] underline underline-offset-2">
              <Orb size={14} />
              Dawnlight Labs
            </button>
          </span>
        </div>
      </div>
      <dl className="selectable m-0 grid grid-cols-[150px_minmax(0,1fr)] gap-x-6 gap-y-3 font-mono text-[12.5px]">
        {(
          [
            ['Studio', `${PRODUCT_NAME} ${STUDIO_VERSION}`],
            ['Engine', health ? `${health.version} at ${ENGINE_URL.replace('http://', '')}` : '…'],
            ['Licence', LICENCE],
          ] as const
        ).map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-muted">{k}</dt>
            <dd className="m-0">{v}</dd>
          </div>
        ))}
      </dl>
    </>
  );
}
