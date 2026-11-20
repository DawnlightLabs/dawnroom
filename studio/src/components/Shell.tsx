// The window chrome: title bar and the slim rail. Both recede so the image gets the room.

import { useEffect, useState, type CSSProperties, type ReactNode } from 'react';
import type { Model, PullProgress } from '../api/types';
import { cancelPull, go, pullModels, useApp, type Page } from '../app/state';
import { PRODUCT_NAME } from '../config';
import { bytes, eta, isRuntimeStep, pausedLabel, pullTotals, runtimeStepLabel, shortGpu, waitLabel } from '../lib/format';
import { appWindow, inTauri, quitApp } from '../lib/tauri';
import { cancelClose, closeNoted, requestClose, restartBlocker, restartToUpdate, useBg } from '../lib/background';
import { MARK, MARK_BODY, MARK_GEOM } from '../lib/brand';
import { Button, cx, Icon, Lockup, Mark, Progress as ProgressBar, StatusDot, useNow, Wordmark, type IconName } from './ui';

export function TitleBar() {
  return (
    <div data-tauri-drag-region className="flex h-9 flex-none items-center justify-between border-b border-line pl-3.5">
      <div data-tauri-drag-region className="pointer-events-none flex items-center text-text" title={PRODUCT_NAME}>
        <Lockup cap={9} />
      </div>
      <div className="flex h-full items-center">
        <UpdateLink />
        {inTauri && (
          <div className="flex h-full text-muted">
            <WindowButton label="Minimise" onClick={appWindow.minimize} path="M0 5H10" />
            <WindowButton label="Maximise" onClick={appWindow.toggleMaximize} path="M.5.5h9v9h-9z" />
            <WindowButton label="Close" onClick={requestClose} path="M0 0L10 10M10 0L0 10" danger />
          </div>
        )}
      </div>
    </div>
  );
}

/** A quiet line in the title bar once a newer Dawnroom has downloaded. Restart installs it, never mid-job or mid-download. */
function UpdateLink() {
  const ready = useBg((s) => s.ready);
  const installing = useBg((s) => s.installing);
  const wait = useApp(restartBlocker);
  if (!ready) return null;
  return (
    <span className="enter mr-3 flex items-center gap-1.5 text-[12px] text-muted">
      <span className="size-[6px] rounded-full bg-safelight-to" aria-hidden="true" />
      {PRODUCT_NAME} {ready} is ready ·
      {installing ? (
        <span>Restarting…</span>
      ) : wait ? (
        <span title="It also installs the next time you quit from the tray.">restart {wait}</span>
      ) : (
        <button type="button" onClick={() => void restartToUpdate()} className="rounded-[4px] text-text underline underline-offset-2 hover:text-text">
          Restart to update
        </button>
      )}
    </span>
  );
}

/** The first time the window closes: one line about the tray, then it hides (or quits, if asked). */
export function CloseNote() {
  const open = useBg((s) => s.closeAsk);
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && cancelClose();
    addEventListener('keydown', onKey);
    return () => removeEventListener('keydown', onKey);
  }, [open]);
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-[60] flex items-end justify-end p-5" onClick={cancelClose}>
      <div role="dialog" aria-label="Dawnroom keeps running" onClick={(e) => e.stopPropagation()} className="enter flex w-[360px] flex-col gap-4 rounded-card border border-line-strong bg-surface p-5 shadow-[0_18px_48px_rgb(0_0_0/0.3)]">
        <p className="m-0 text-[13.5px] leading-snug text-text">{PRODUCT_NAME} keeps running in the tray, so downloads and agents carry on. Quit it from the tray icon.</p>
        <div className="flex justify-end gap-2">
          <Button size="sm" onClick={() => (closeNoted(false), void quitApp())}>
            Quit {PRODUCT_NAME}
          </Button>
          <Button size="sm" variant="solid" autoFocus onClick={() => closeNoted(true)}>
            Hide to tray
          </Button>
        </div>
      </div>
    </div>
  );
}

function WindowButton({ label, onClick, path, danger }: { label: string; onClick: () => void; path: string; danger?: boolean }) {
  return (
    <button type="button" aria-label={label} title={label} tabIndex={-1} onClick={onClick} className={cx('flex h-full w-[46px] items-center justify-center transition-colors hover:text-text', danger ? 'hover:bg-[#c42b1c] hover:text-white' : 'hover:bg-raised')}>
      <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
        <path d={path} stroke="currentColor" />
      </svg>
    </button>
  );
}

const NAV: { page: Page; label: string; icon: IconName }[] = [
  { page: 'create', label: 'Create', icon: 'create' },
  { page: 'library', label: 'Library', icon: 'library' },
  { page: 'models', label: 'Models', icon: 'models' },
  { page: 'settings', label: 'Settings', icon: 'settings' },
];

/** A 56 px icon rail. Labels show on hover and focus; downloads and the GPU sit at the foot, in words. */
export function Rail() {
  const page = useApp((s) => s.page);
  return (
    <nav aria-label="Main" className="flex w-14 flex-none flex-col items-center gap-1 border-r border-line py-3">
      {NAV.map((n) => (
        <button
          key={n.page}
          type="button"
          aria-label={n.label}
          aria-current={page === n.page ? 'page' : undefined}
          onClick={() => go(n.page)}
          className={cx('group relative flex size-10 items-center justify-center rounded-control transition-colors', page === n.page ? 'bg-surface text-text' : 'text-muted hover:bg-surface hover:text-text')}
        >
          <Icon name={n.icon} size={18} />
          <Tip>{n.label}</Tip>
        </button>
      ))}
      <div className="flex-1" />
      <div className="flex flex-col items-center gap-1 border-t border-line pt-2">
        <DownloadsIndicator />
        <GpuIndicator />
      </div>
    </nav>
  );
}

/** A label that appears beside a rail item on hover or keyboard focus. */
function Tip({ children }: { children: ReactNode }) {
  return (
    <span className="pointer-events-none absolute left-full z-40 ml-2 hidden rounded-[6px] border border-line-strong bg-raised px-2 py-1 text-left text-[12px] whitespace-nowrap text-text group-hover:block group-focus-visible:block">
      {children}
    </span>
  );
}

/** A small panel beside the rail's foot. Closes on Escape or a click outside. */
function RailPopover({ label, onClose, children }: { label: string; onClose: () => void; children: ReactNode }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    addEventListener('keydown', onKey);
    return () => removeEventListener('keydown', onKey);
  }, [onClose]);
  return (
    <>
      <div className="fixed inset-0 z-40" onClick={onClose} aria-hidden="true" />
      <div role="dialog" aria-label={label} className="enter absolute bottom-0 left-full z-50 ml-2 flex w-[320px] flex-col gap-3 rounded-card border border-line-strong bg-surface p-4 text-left shadow-[0_18px_48px_rgb(0_0_0/0.3)]">
        {children}
      </div>
    </>
  );
}

const footButton = 'group relative flex w-12 flex-col items-center gap-1 rounded-control py-2 text-muted transition-colors hover:bg-surface hover:text-text';

/** A ring that fills with the download (or sits dashed while paused or waiting). */
function Ring({ fraction, paused }: { fraction: number; paused: boolean }) {
  const r = 9;
  const c = 2 * Math.PI * r;
  return (
    <span className="relative flex size-[22px] items-center justify-center">
      <svg width="22" height="22" viewBox="0 0 22 22" className="absolute inset-0 -rotate-90" aria-hidden="true">
        <circle cx="11" cy="11" r={r} fill="none" stroke="var(--color-line-strong)" strokeWidth="1.5" strokeDasharray={paused ? '2 2.6' : undefined} />
        {!paused && <circle cx="11" cy="11" r={r} fill="none" stroke="var(--color-safelight-to)" strokeWidth="1.5" strokeDasharray={`${c * fraction} ${c}`} className="transition-[stroke-dasharray] duration-500" />}
      </svg>
      <Icon name="download" size={11} />
    </span>
  );
}

/** Shows while something downloads, waits for the network, or sits paused; the popover lists each one. */
function DownloadsIndicator() {
  const pulls = useApp((s) => s.pulls);
  const models = useApp((s) => s.models);
  const queue = useApp((s) => s.pullQueue);
  const runtimePaused = useApp((s) => s.runtimePaused);
  const [open, setOpen] = useState(false);
  useNow();
  const active = models.filter((m) => m.state === 'pulling' || m.id in pulls);
  const queued = queue.map((id) => models.find((m) => m.id === id)).filter((m) => m != null && !active.includes(m)) as Model[];
  const paused = models.filter((m) => m.paused && !(m.id in pulls) && m.state !== 'pulling');
  const count = active.length + queued.length + paused.length + (runtimePaused ? 1 : 0);
  if (!count) return null;
  const first = active[0];
  const p = first ? (pulls[first.id] ?? first.pull) : null;
  const fraction = first ? pullTotals(first, p).fraction : 0;
  const waiting = p?.stage === 'waiting';
  const short = first ? (waiting ? 'Wait' : `${Math.round(fraction * 100)}%`) : 'Paused';
  const summary = first ? `${first.name}, ${waiting ? 'waiting for the network' : `${Math.round(fraction * 100)} percent`}` : `${paused.length + (runtimePaused ? 1 : 0)} paused`;
  return (
    <div className="relative">
      <button type="button" aria-expanded={open} aria-label={`Downloads: ${summary}. Show downloads.`} onClick={() => setOpen((o) => !o)} className={cx(footButton, open && 'bg-surface text-text')}>
        <Ring fraction={fraction} paused={!first || waiting} />
        <span className="text-[10px] leading-none font-medium">{short}</span>
      </button>
      {open && (
        <RailPopover label="Downloads" onClose={() => setOpen(false)}>
          <div className="flex items-baseline justify-between">
            <h2 className="m-0 text-[13.5px] font-semibold text-text">Downloads</h2>
            <span className="edge text-muted">{count}</span>
          </div>
          {active.map((m) => (
            <DownloadRow key={m.id} model={m} progress={pulls[m.id] ?? m.pull} />
          ))}
          {queued.map((m) => (
            <div key={m.id} className="flex items-baseline justify-between gap-3 text-[12.5px]">
              <span className="truncate text-text">{m.name}</span>
              <span className="edge flex-none text-muted">next · {bytes(m.download_bytes || m.size_bytes)}</span>
            </div>
          ))}
          {paused.map((m) => (
            <div key={m.id} className="flex flex-col gap-1.5">
              <div className="flex items-baseline justify-between gap-3 text-[12.5px]">
                <span className="truncate text-text">{m.name}</span>
                <span className="edge flex-none text-muted">paused</span>
              </div>
              <span className="text-[12px] leading-snug text-muted">{m.paused && pausedLabel(m.paused)}</span>
              <Button size="sm" icon="download" className="self-start" onClick={() => pullModels([m.id])}>
                Resume
              </Button>
            </div>
          ))}
          {runtimePaused && <p className="m-0 text-[12px] leading-snug text-muted">The image runtime: {pausedLabel(runtimePaused).toLowerCase()} It resumes with the next model download.</p>}
          <button type="button" onClick={() => (setOpen(false), go('models'))} className="flex items-center justify-between border-t border-line pt-3 text-[12.5px] text-muted hover:text-text">
            Open Models
            <Icon name="next" size={14} />
          </button>
        </RailPopover>
      )}
    </div>
  );
}

function DownloadRow({ model, progress }: { model: Model; progress: PullProgress | null }) {
  const t = pullTotals(model, progress);
  const runtime = progress && isRuntimeStep(progress);
  const line = !progress
    ? 'Starting…'
    : progress.stage === 'waiting' && progress.wait
      ? waitLabel(progress.wait)
      : runtime
        ? runtimeStepLabel(progress)
        : progress.stage === 'testing'
          ? 'Making a test image…'
          : progress.stage === 'verifying' || progress.stage === 'present'
            ? 'Checking files…'
            : [`${bytes(t.done)} of ${bytes(t.total)}`, progress.speed_bps > 0 ? `${bytes(progress.speed_bps)}/s` : null, progress.speed_bps > 0 && t.total > t.done ? `${eta((t.total - t.done) / progress.speed_bps)} left` : null].filter(Boolean).join(' · ');
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-baseline justify-between gap-3 text-[12.5px]">
        <span className="truncate text-text">
          {model.name}
          {model.tag && <span className="edge text-muted"> {model.tag}</span>}
        </span>
        <span className="edge flex-none text-muted">{Math.round(t.fraction * 100)}%</span>
      </div>
      <ProgressBar value={t.fraction} label={`${model.name} download`} />
      <div className="flex items-center justify-between gap-3">
        <span className="edge truncate text-muted">{line}</span>
        <button type="button" onClick={() => void cancelPull(model.id)} className="flex-none rounded-[4px] text-[12px] text-muted underline underline-offset-2 hover:text-text">
          Pause
        </button>
      </div>
    </div>
  );
}

/** The GPU in words at the rail's foot: a meter of its memory and the numbers; the popover says the rest. */
function GpuIndicator() {
  const gpu = useApp((s) => s.health?.gpu ?? s.gpu);
  const health = useApp((s) => s.health);
  const models = useApp((s) => s.models);
  const hardware = useApp((s) => s.hardware);
  const [open, setOpen] = useState(false);
  if (!gpu) return null;
  const loaded = models.find((m) => m.id === health?.model);
  const state = health?.busy ? 'developing' : loaded ? 'loaded' : 'idle';
  const used = gpu.vram_used_mib / 1024;
  const total = gpu.vram_total_mib / 1024;
  const share = Math.min(1, gpu.vram_used_mib / Math.max(1, gpu.vram_total_mib));
  const what = state === 'developing' ? `Developing with ${loaded?.name ?? 'a model'}` : loaded ? `${loaded.name} is loaded` : 'Nothing loaded';
  const ram = hardware?.ram;
  const choice = hardware?.choice;
  return (
    <div className="relative">
      <button type="button" aria-expanded={open} aria-label={`GPU ${shortGpu(gpu.name)}: ${used.toFixed(1)} of ${total.toFixed(0)} GB in use. ${what}. Show details.`} onClick={() => setOpen((o) => !o)} className={cx(footButton, open && 'bg-surface text-text')}>
        <span className="flex items-center gap-1 text-[10px] leading-none font-medium">
          <span className={cx('size-[6px] flex-none rounded-full', state === 'developing' ? 'breathe bg-safelight-to' : state === 'loaded' ? 'bg-ok' : 'border border-muted')} />
          GPU
        </span>
        <span className="h-[4px] w-8 overflow-hidden rounded-full bg-line-strong" aria-hidden="true">
          <span className={cx('block h-full rounded-full', state === 'developing' ? 'bg-safelight' : 'bg-text/70')} style={{ width: `${Math.max(4, share * 100)}%` }} />
        </span>
        <span className="edge text-[10px] leading-none">
          {used.toFixed(1)}/{total.toFixed(0)}
        </span>
      </button>
      {open && (
        <RailPopover label="GPU" onClose={() => setOpen(false)}>
          <div className="flex flex-col gap-0.5">
            <h2 className="m-0 text-[13.5px] font-semibold text-text">{shortGpu(gpu.name)}</h2>
            <span className="edge text-muted">{gpu.name}</span>
          </div>
          <div className="flex flex-col gap-1.5">
            <div className="flex items-baseline justify-between text-[12.5px]">
              <span className="text-text">Graphics memory</span>
              <span className="edge text-muted">
                {used.toFixed(1)} of {total.toFixed(1)} GB in use
              </span>
            </div>
            <span className="h-[5px] overflow-hidden rounded-full bg-line-strong" aria-hidden="true">
              <span className={cx('block h-full rounded-full', state === 'developing' ? 'bg-safelight' : 'bg-text/70')} style={{ width: `${Math.max(2, share * 100)}%` }} />
            </span>
            <span className="text-[12px] leading-snug text-muted">Counts every app on this GPU, not only Dawnroom.</span>
          </div>
          <div className="flex items-center gap-2 text-[12.5px] text-text">
            <StatusDot state={state === 'developing' ? 'busy' : state === 'loaded' ? 'ok' : 'off'} />
            {what}
          </div>
          {state === 'loaded' && <p className="-mt-1.5 m-0 text-[12px] leading-snug text-muted">It stays in memory for the next image and unloads after a few idle minutes.</p>}
          {ram && (
            <div className="flex items-baseline justify-between text-[12.5px]">
              <span className="text-text">System memory</span>
              <span className="edge text-muted">
                {(ram.total_bytes / 2 ** 30).toFixed(0)} GB
              </span>
            </div>
          )}
          {choice?.build && (
            <div className="flex items-baseline justify-between text-[12.5px]">
              <span className="text-text">Runtime</span>
              <span className="edge text-muted">
                {choice.build}
                {choice.experimental ? ' · experimental' : ''}
              </span>
            </div>
          )}
          {choice?.message && (
            <p className="m-0 text-[12px] leading-snug text-text">
              {choice.message}{' '}
              {choice.link && (
                <a href={choice.link} target="_blank" rel="noreferrer" className="text-link underline underline-offset-2">
                  Get the driver
                </a>
              )}
            </p>
          )}
        </RailPopover>
      )}
    </div>
  );
}

/** A full-window message: the engine is starting, or something is in the way. */
export function Splash({ title, children }: { title?: string; children?: ReactNode }) {
  return (
    <div className="enter flex flex-1 flex-col items-center justify-center gap-5 px-8 text-center">
      <Mark size={56} />
      {title && <h1 className="m-0 font-display text-[44px] leading-[1.1] font-light tracking-[-0.02em]">{title}</h1>}
      {children}
    </div>
  );
}

/**
 * The brand opening, played only while the engine actually starts: the sun sweep rises into the
 * Dawnroom tile over a rising horizon (parallax), then the wordmark. A body-only tile covers the mark,
 * clipped to a circle on the tile's deep core that shrinks from the far corner to the cut; at rest the
 * cover is hidden and only the mark shows. It is unmounted the moment the engine
 * answers, so it never outlasts the real wait; the app's own 220 ms enter is the tail. A short
 * delay keeps it from flashing when the engine is already up. Still lockup with motion off.
 */
const px = (share: number) => `${(share * 52).toFixed(2)}px`;
const OPENING_VARS = { '--cx': px(MARK_GEOM.cx), '--cy': px(MARK_GEOM.cy), '--cut': px(MARK_GEOM.cut), '--far': px(MARK_GEOM.far) } as CSSProperties;

export function Opening({ children }: { children?: ReactNode }) {
  return (
    <div className="opening flex flex-1 flex-col items-center justify-center gap-8 px-8 text-center" aria-label="Dawnroom">
      <div className="opening-stage" aria-hidden="true">
        <div className="opening-glow" />
        <div className="opening-horizon" />
        <div className="opening-lockup">
          <span className="opening-mark" style={OPENING_VARS}>
            <img src={MARK} width={52} height={52} alt="" draggable={false} />
            <img className="opening-under" src={MARK_BODY} width={52} height={52} alt="" draggable={false} />
          </span>
          <span className="opening-word text-text">
            <Wordmark height={40} />
          </span>
        </div>
      </div>
      <div className="opening-status">{children}</div>
    </div>
  );
}
