/// First run: a few steps on a flush-left rail, each showing its reading (the GPU, the drive, the
// model, sign-in), one step at a time beside it. Then the download, with the first prompt typed
// while it comes; it develops when the model is ready.

import { useEffect, useState, type ReactNode } from 'react';
import { api, awaitRestart, cancelPull, develop, discardPaused, isDeveloping, pullModels, setDraft, stopGroup, useApp } from '../app/state';
import type { Drive, Model } from '../api/types';
import { DevelopButton } from '../components/Composer';
import { DawnProgress } from '../components/Moments';
import { Splash } from '../components/Shell';
import { Button, ConfirmLink, cx, HowLink, Icon, Notice, Progress, StatusDot, useNow } from '../components/ui';
import { DOCS, ENGINE_URL, PRODUCT_NAME } from '../config';
import { setSignIn, useBg } from '../lib/background';
import { bytes, eta, isRuntimeStep, pausedLabel, pullTotals, runtimeStepLabel, shortGpu, vram, waitLabel } from '../lib/format';
import { fitFor, memoryShort } from '../lib/models';
import { inTauri, openExternal, pickFolder } from '../lib/tauri';
import { LicenceSheet } from './Models';

/** Kept free on top of a download, same as the engine (engine/src/disk.rs). */
const RESERVE = 500_000_000;
type Step = 'machine' | 'where' | 'model' | 'signin';
const STEP_KEY = 'studio.firstrun.step';
/** A drive by its label and letter: "Windows (C:)". */
const driveName = (d: Drive) => {
  const root = d.root.replace(/[\\/]+$/, '');
  return d.label ? `${d.label} (${root})` : root || '/';
};

export function FirstRun() {
  const gpu = useApp((s) => s.gpu)!;
  const models = useApp((s) => s.models);
  const pulls = useApp((s) => s.pulls);
  const queue = useApp((s) => s.pullQueue);
  const notice = useApp((s) => s.notice);
  const runtimeNeeded = useApp((s) => s.runtimeNeeded);
  const hardware = useApp((s) => s.hardware);
  const runtimePaused = useApp((s) => s.runtimePaused);
  const auto = useApp((s) => s.auto);
  const signIn = useBg((s) => s.signIn);

  // The step survives the engine restarting into a new data folder (the page remounts).
  const [step, setStepState] = useState<Step>(() => {
    try {
      return (sessionStorage.getItem(STEP_KEY) as Step | null) ?? 'machine';
    } catch {
      return 'machine';
    }
  });
  const setStep = (x: Step) => {
    try {
      sessionStorage.setItem(STEP_KEY, x);
    } catch {
      // private mode: the step is forgotten on a remount
    }
    setStepState(x);
  };
  const [drives, setDrives] = useState<{ data_dir: string; drives: Drive[] } | null>(null);
  const [where, setWhere] = useState<string | null>(null);
  const [moveError, setMoveError] = useState<string | null>(null);
  const [alsoEdit, setAlsoEdit] = useState(false);
  const [sheet, setSheet] = useState<Model | null>(null);

  useEffect(() => {
    api.drives().then(
      (d) => (setDrives(d), setWhere((w) => w ?? d.data_dir)),
      () => setDrives(null),
    );
  }, []);

  const byId = (id?: string | null) => models.find((m) => m.id === id);
  const gen = auto?.generate && !auto.generate.ok ? byId(auto.generate.suggest?.model) : undefined;
  const edit = auto?.edit && !auto.edit.ok ? byId(auto.edit.suggest?.model) : undefined;
  const editExtra = edit && edit.id !== gen?.id ? edit : undefined;
  const chosen = [gen, alsoEdit ? editExtra : undefined].filter((m): m is Model => !!m);
  const modelBytes = chosen.reduce((n, m) => n + m.download_bytes, 0);
  const needed = modelBytes + (modelBytes > 0 ? runtimeNeeded : 0);
  const current = drives?.drives.find((d) => d.current);
  const target = drives?.drives.find((d) => d.path === where) ?? (where && where !== drives?.data_dir ? null : current);
  const fits = !target || needed + RESERVE <= target.free_bytes;

  const moving = models.filter((m) => m.state === 'pulling' || queue.includes(m.id) || m.id in pulls);
  const pausedModel = models.find((m) => m.paused);
  const paused = moving.length === 0 ? (pausedModel?.paused ?? runtimePaused) : null;
  const choice = hardware?.choice;

  const download = () => {
    if (!chosen.length) return;
    setDraft({ task: 'generate' });
    const gated = chosen.find((m) => m.licence.accept_required && m.licence_accepted_ms == null);
    if (gated) setSheet(gated);
    else pullModels(chosen.map((m) => m.id));
  };

  /** Continue from "Where models live": another folder means the engine moves its (still empty) data there and restarts. */
  const confirmWhere = async () => {
    setMoveError(null);
    if (!where || !drives || where === drives.data_dir) return setStep('model');
    try {
      setStep('model');
      await api.startMove(where);
      awaitRestart();
    } catch (e) {
      setStep('where');
      setMoveError(e instanceof Error ? e.message : String(e));
    }
  };
  const other = async () => {
    const picked = await pickFolder('Where should Dawnroom keep models and images?');
    if (picked) setWhere(picked);
  };

  const steps: { id: Step; label: string; reading: ReactNode }[] = [
    { id: 'machine', label: 'Your machine', reading: `${shortGpu(gpu.name)} · ${vram(gpu.vram_total_mib)}` },
    { id: 'where', label: 'Where models live', reading: target ? `${driveName(target)} · ${bytes(target.free_bytes)} free` : (where ?? '…') },
    { id: 'model', label: 'Your first model', reading: gen ? `${gen.name}${alsoEdit && editExtra ? ` + ${editExtra.name}` : ''} · ${bytes(needed)}` : '…' },
    ...(inTauri ? [{ id: 'signin' as Step, label: 'At sign-in', reading: signIn ? 'starts in the tray' : 'off' }] : []),
  ];
  const at = Math.max(0, steps.findIndex((x) => x.id === step));
  const started = moving.length > 0;
  const last = at === steps.length - 1;

  return (
    <div className="flex min-h-0 flex-1">
      {/* The rail: every step with its reading. Earlier steps can be revisited until the download starts. */}
      <nav aria-label="Setup" className="flex w-[280px] flex-none flex-col border-r border-line pt-8 pb-5">
        <h1 className="m-0 px-7 pb-1 font-display text-[30px] leading-[1.1] font-light tracking-[-0.02em]">Set up</h1>
        <p className="m-0 px-7 pb-6 text-[12.5px] leading-snug text-muted">Everything runs on this machine. No account, no cloud, no credits.</p>
        <ol className="m-0 flex list-none flex-col p-0">
          {steps.map((x, i) => {
            const done = started || i < at;
            const here = !started && i === at;
            return (
              <li key={x.id}>
                <button
                  type="button"
                  disabled={started || i > at}
                  aria-current={here ? 'step' : undefined}
                  onClick={() => setStep(x.id)}
                  className={cx('relative flex w-full items-start gap-3 px-7 py-3 text-left transition-colors', here ? 'bg-surface' : 'enabled:hover:bg-surface/60')}
                >
                  <span className={cx('absolute inset-y-2 left-0 w-[3px] rounded-r-full bg-safelight transition-opacity', here ? 'opacity-100' : 'opacity-0')} aria-hidden="true" />
                  <span className={cx('edge mt-px flex size-5 flex-none items-center justify-center rounded-full border text-[10.5px]', done ? 'border-text bg-text text-bg' : here ? 'border-text text-text' : 'border-line-strong text-muted')}>
                    {done ? <Icon name="check" size={11} stroke={2.2} /> : i + 1}
                  </span>
                  <span className="flex min-w-0 flex-col gap-0.5">
                    <span className={cx('text-[13.5px]', here || done ? 'text-text' : 'text-muted')}>{x.label}</span>
                    {(i <= at || started) && <span className="edge truncate text-[11px] text-muted">{x.reading}</span>}
                  </span>
                </button>
              </li>
            );
          })}
          <li className={cx('relative flex items-start gap-3 px-7 py-3', started ? 'bg-surface text-text' : 'text-muted')}>
            <span className={cx('absolute inset-y-2 left-0 w-[3px] rounded-r-full bg-safelight transition-opacity', started ? 'opacity-100' : 'opacity-0')} aria-hidden="true" />
            <span className={cx('edge mt-px flex size-5 flex-none items-center justify-center rounded-full border text-[10.5px]', started ? 'border-text' : 'border-line-strong')}>{steps.length + 1}</span>
            <span className="text-[13.5px]">Download and develop</span>
          </li>
        </ol>
      </nav>

      <main className="flex min-w-0 flex-1 flex-col overflow-y-auto">
        <div key={started ? 'download' : step} className="enter flex min-h-full w-full max-w-[760px] flex-col gap-6 px-12 pt-10 pb-6">
          {started ? (
            <>
              <StepHead title="Downloading" line="Downloaded once, checked, then it works offline. Write your first prompt while it comes." />
              <div className="flex flex-col gap-3.5 rounded-card border border-line bg-surface px-4 py-3.5">
                <Dawn model={moving.find((m) => !queue.includes(m.id)) ?? moving[0]} />
                {moving.map((m) => (
                  <Download key={m.id} model={m} waiting={queue.includes(m.id)} />
                ))}
              </div>
            </>
          ) : step === 'machine' ? (
            <>
              <StepHead title="Your machine" line={`${PRODUCT_NAME} makes images on this graphics card, and uses system memory too when a model needs more.`} docs={DOCS.offload} />
              <div className="grid grid-cols-[repeat(auto-fit,minmax(200px,1fr))] gap-3">
                <Fact k="Graphics card" v={shortGpu(gpu.name)} sub={`${vram(gpu.vram_total_mib)} of graphics memory`} dot={choice && choice.status !== 'ok' ? 'off' : 'ok'} />
                {hardware?.ram && <Fact k="System memory" v={`${Math.round(hardware.ram.total_bytes / 2 ** 30)} GB`} sub="Used when a model doesn't fit on the card" />}
                {choice?.build && <Fact k="Runtime" v={choice.build.includes('cuda') ? 'CUDA 12' : choice.build.includes('vulkan') ? 'Vulkan' : choice.build} sub={choice.experimental ? 'Experimental on this card' : 'Downloaded with your first model'} />}
              </div>
              {choice?.experimental && choice.status === 'ok' && <p className="m-0 text-[12.5px] text-muted">{choice.reason}</p>}
              {choice && choice.status !== 'ok' && (
                <Notice action={choice.link ? <Button onClick={() => openExternal(choice.link!)}>Get the driver</Button> : undefined}>{choice.message ?? choice.reason}</Notice>
              )}
            </>
          ) : step === 'where' ? (
            <>
              <StepHead title="Where models live" line="Models are big, a few GB each, so pick a drive with room to spare. Your images go in the same folder." docs={DOCS.storage} />
              {drives ? (
                <div role="radiogroup" aria-label="Drive" className="flex flex-col gap-2.5">
                  {drives.drives.map((d) => (
                    <DriveRow key={d.root} d={d} on={where === d.path} need={(gen?.download_bytes ?? 0) + runtimeNeeded} onPick={() => setWhere(d.path)} />
                  ))}
                  {where && !drives.drives.some((d) => d.path === where) && (
                    <div className="flex items-center gap-3 rounded-card border border-line-strong bg-raised px-4 py-3 text-[13px]">
                      <Icon name="folder" size={15} />
                      <span className="selectable min-w-0 flex-1 truncate font-mono text-[12.5px]">{where}</span>
                    </div>
                  )}
                </div>
              ) : (
                <p className="edge m-0 text-muted">Reading your drives…</p>
              )}
              <div className="flex flex-wrap items-center gap-x-4 gap-y-2 text-[12.5px] text-muted">
                {inTauri ? (
                  <button type="button" onClick={() => void other()} className="flex items-center gap-1.5 rounded-[4px] underline underline-offset-2 hover:text-text">
                    <Icon name="folder" size={13} />
                    Choose another folder
                  </button>
                ) : (
                  <label className="flex min-w-[320px] flex-1 items-center gap-2">
                    Or a folder:
                    <input value={where ?? ''} onChange={(e) => setWhere(e.target.value)} className="selectable h-8 min-w-0 flex-1 rounded-control border border-line bg-surface px-2 font-mono text-[12px] text-text" />
                  </label>
                )}
                <span>You can move everything later in Settings → Storage.</span>
              </div>
              {moveError && <Notice>{moveError}</Notice>}
            </>
          ) : step === 'model' ? (
            <>
              <StepHead title="Your first model" line="Auto picked the model that suits this machine best. Add others any time in Models." docs={DOCS.auto} />
              {paused && (
                <div className="flex items-center justify-between gap-4 rounded-card border border-line-strong bg-surface px-4 py-3" data-paused>
                  <span className="flex flex-col gap-1">
                    <span className="text-[14px]">{pausedLabel(paused)}</span>
                    <span className="text-[12.5px] text-muted">{pausedModel ? pausedModel.name : 'The image runtime'} is kept on disk. Resume picks up where it stopped.</span>
                    <span className="pt-0.5">
                      <ConfirmLink label="Discard" confirm="Discard it? The download starts again from zero." onConfirm={() => void discardPaused(pausedModel ? pausedModel.id : 'runtime')} />
                    </span>
                  </span>
                  {(pausedModel ?? gen) && (
                    <Button variant="solid" icon="download" onClick={() => pullModels([(pausedModel ?? gen)!.id])} className="h-9 flex-none rounded-card px-4 text-[14px]">
                      Resume
                    </Button>
                  )}
                </div>
              )}
              {gen ? <ModelChoice m={gen} task="generate" lead="Makes images" checked /> : <Notice>{auto?.generate && !auto.generate.ok ? auto.generate.error : 'Asking the engine which model suits this machine…'}</Notice>}
              {editExtra && (
                <label className="cursor-pointer">
                  <input type="checkbox" checked={alsoEdit} onChange={(e) => setAlsoEdit(e.target.checked)} className="peer sr-only" />
                  <ModelChoice m={editExtra} task="edit" lead="Also get an editing model" checked={alsoEdit} optional />
                </label>
              )}
              <p className="m-0 text-[12.5px] leading-snug text-muted">
                Needs <span className="text-text">{bytes(needed)}</span>
                {modelBytes > 0 && runtimeNeeded > 0 && (
                  <>
                    {' '}
                    ({bytes(modelBytes)} of models, {bytes(runtimeNeeded)} for the image runtime)
                  </>
                )}
                {target && (
                  <>
                    {' '}
                    on {driveName(target)}, which has {bytes(target.free_bytes)} free
                  </>
                )}
                . <HowLink href={DOCS.sizeTags} label="About sizes and licences" />
              </p>
              {!fits && target && (
                <Notice>
                  Not enough space: this needs {bytes(needed)} and {driveName(target)} has {bytes(target.free_bytes)} free ({PRODUCT_NAME} keeps {bytes(RESERVE)} spare). Pick another drive, or free up {bytes(needed + RESERVE - target.free_bytes)}.
                </Notice>
              )}
            </>
          ) : (
            <>
              <StepHead title="Start Dawnroom when you sign in?" line="It opens quietly in the tray, so agents can reach it and downloads carry on. You can change this in Settings." docs={DOCS.tray} />
              <div role="radiogroup" aria-label="Start at sign-in" className="grid grid-cols-2 gap-3">
                {[
                  { on: false, title: 'Not now', line: 'Open it from the Start menu when you need it.' },
                  { on: true, title: 'Yes, start in the tray', line: 'A small tile by the clock. Quit from its menu any time.' },
                ].map((o) => (
                  <button key={String(o.on)} type="button" role="radio" aria-checked={!!signIn === o.on} onClick={() => void setSignIn(o.on)} className={cx('flex flex-col gap-1.5 rounded-card border p-4 text-left transition-colors', !!signIn === o.on ? 'border-line-strong bg-raised' : 'border-line bg-surface hover:bg-raised')}>
                    <span className="flex items-center justify-between font-medium">
                      {o.title}
                      {!!signIn === o.on && <Icon name="check" stroke={2} />}
                    </span>
                    <span className="text-[12.5px] leading-snug text-muted">{o.line}</span>
                  </button>
                ))}
              </div>
            </>
          )}

          {notice && <Notice>{notice.text}</Notice>}
          <div className="flex-1" />
          {started ? (
            <FirstPrompt ready model={moving[0]} />
          ) : (
            <div className="flex items-center justify-between gap-4 border-t border-line pt-4">
              {at > 0 ? (
                <Button variant="ghost" icon="back" onClick={() => setStep(steps[at - 1].id)}>
                  Back
                </Button>
              ) : (
                <span />
              )}
              {step === 'where' ? (
                <Button variant="solid" onClick={() => void confirmWhere()} disabled={!where} className="h-9 rounded-card px-4 text-[14px]">
                  {where && drives && where !== drives.data_dir ? 'Use this folder' : 'Continue'}
                </Button>
              ) : last ? (
                <Button variant="solid" icon="download" onClick={download} disabled={!gen || !fits} className="h-9 rounded-card px-4 text-[14px]">
                  Download · {bytes(needed)}
                </Button>
              ) : (
                <Button variant="solid" onClick={() => setStep(steps[at + 1].id)} disabled={step === 'model' && (!gen || !fits)} className="h-9 rounded-card px-4 text-[14px]">
                  Continue
                </Button>
              )}
            </div>
          )}
        </div>
      </main>
      {sheet && <LicenceSheet model={sheet} onClose={() => setSheet(null)} onAccepted={() => pullModels(chosen.map((m) => m.id))} />}
    </div>
  );
}

/** A step's title, one line, and (where a feature needs it) a quiet docs link. */
function StepHead({ title, line, docs }: { title: string; line: string; docs?: string }) {
  return (
    <header className="flex flex-col gap-2">
      <h2 className="m-0 font-display text-[40px] leading-[1.08] font-light tracking-[-0.02em]">{title}</h2>
      <p className="m-0 max-w-[62ch] text-[14px] leading-snug text-muted">
        {line}
        {docs && (
          <>
            {' '}
            <HowLink href={docs} />
          </>
        )}
      </p>
    </header>
  );
}

function Fact({ k, v, sub, dot }: { k: string; v: string; sub: string; dot?: 'ok' | 'off' }) {
  return (
    <div className="flex flex-col gap-1 rounded-card border border-line bg-surface px-4 py-3.5">
      <span className="eyebrow text-muted">{k}</span>
      <span className="flex items-center gap-2 text-[15px] font-medium">
        {dot && <StatusDot state={dot} />}
        {v}
      </span>
      <span className="text-[12px] text-muted">{sub}</span>
    </div>
  );
}

/** One drive as a bar: what's used, what's free, and (on the picked one) what the first download takes. */
function DriveRow({ d, on, need, onPick }: { d: Drive; on: boolean; need: number; onPick: () => void }) {
  const used = d.total_bytes - d.free_bytes;
  const pct = (n: number) => `${Math.max(0, Math.min(100, (n / Math.max(1, d.total_bytes)) * 100))}%`;
  const room = need + RESERVE <= d.free_bytes;
  return (
    <button type="button" role="radio" aria-checked={on} onClick={onPick} className={cx('flex flex-col gap-2.5 rounded-card border px-4 py-3.5 text-left transition-colors', on ? 'border-line-strong bg-raised' : 'border-line bg-surface hover:bg-raised')}>
      <span className="flex items-baseline justify-between gap-4">
        <span className="flex items-center gap-2 text-[14px] font-medium">
          <span className={cx('flex size-[14px] flex-none items-center justify-center rounded-full border-[1.5px]', on ? 'border-text' : 'border-line-strong')} aria-hidden="true">
            {on && <span className="size-[6px] rounded-full bg-text" />}
          </span>
          {driveName(d)}
          {d.current && <span className="edge text-[11px] font-normal text-muted">now</span>}
        </span>
        <span className={cx('edge text-[12px]', room ? 'text-muted' : 'text-danger')}>
          {bytes(d.free_bytes)} free of {bytes(d.total_bytes)}
        </span>
      </span>
      <span className="flex h-[6px] overflow-hidden rounded-full bg-line-strong" aria-hidden="true">
        <span className="h-full bg-text/45" style={{ width: pct(used) }} />
        {on && need > 0 && <span className="h-full bg-safelight" style={{ width: pct(need) }} />}
      </span>
      <span className="selectable truncate font-mono text-[11.5px] text-muted">{d.path}</span>
    </button>
  );
}

/** A model offered in first run: what it does, its size, how it runs here, its licence. */
function ModelChoice({ m, task, lead, checked, optional }: { m: Model; task: 'generate' | 'edit'; lead: string; checked: boolean; optional?: boolean }) {
  const fit = fitFor(m, task);
  const licence = m.licence.commercial && !m.licence.use_restrictions ? 'commercial use allowed' : (m.licence.name ?? m.licence.spdx);
  return (
    <div className={cx('flex items-start gap-4 rounded-card border px-4 py-3.5 transition-colors peer-focus-visible:outline-2 peer-focus-visible:outline-ring', checked ? 'border-line-strong bg-raised' : 'border-line bg-surface hover:bg-raised')}>
      <span className={cx('mt-0.5 flex size-[16px] flex-none items-center justify-center border-[1.5px]', optional ? 'rounded-[4px]' : 'rounded-full', checked ? 'border-text bg-text text-bg' : 'border-line-strong')} aria-hidden="true">
        {checked && <Icon name="check" size={11} stroke={2.4} />}
      </span>
      <span className="flex min-w-0 flex-1 flex-col gap-1">
        <span className="eyebrow text-muted">{lead}</span>
        <span className="flex items-baseline gap-2 text-[16px] font-medium">
          {m.name}
          {m.tag && <span className="edge text-[11.5px] font-normal text-muted">{m.tag}</span>}
        </span>
        <span className="text-[13px] leading-snug text-muted">{m.tagline}</span>
        <span className="edge pt-1 text-[11.5px] text-muted">
          {bytes(m.download_bytes)} to download · {memoryShort(fit)} · {licence}
        </span>
      </span>
    </div>
  );
}

/** The download as the tile rising. With reduced motion it's hidden and the plain bars below carry the progress. */
function Dawn({ model }: { model: Model }) {
  const progress = useApp((s) => s.pulls[model.id] ?? model.pull);
  return (
    <div className="dawn-wrap mx-auto w-full max-w-[400px]">
      <DawnProgress value={pullTotals(model, progress).fraction} label={`${model.name} download`} />
    </div>
  );
}

function Download({ model, waiting }: { model: Model; waiting: boolean }) {
  const progress = useApp((s) => s.pulls[model.id] ?? model.pull);
  const t = pullTotals(model, progress);
  const downloading = progress?.stage === 'downloading' && progress.speed_bps > 0;
  const now = useNow();
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-baseline justify-between gap-4">
        <span className={cx('text-[13px]', waiting && 'text-muted')}>{model.name}</span>
        <span className="edge flex items-baseline gap-3 text-[11.5px] text-muted" aria-live="polite">
          {waiting
            ? `Next · ${bytes(model.download_bytes || model.size_bytes)}`
            : progress?.stage === 'waiting' && progress.wait
              ? waitLabel(progress.wait, now)
              : progress && isRuntimeStep(progress)
              ? runtimeStepLabel(progress)
              : progress?.stage === 'verifying'
                ? 'Checking the download…'
                : !progress
                  ? 'Starting…'
                  : `${bytes(t.done)} of ${bytes(t.total)}${downloading ? ` · ${eta((t.total - t.done) / progress.speed_bps)}` : ''}`}
          {!waiting && (
            <button type="button" onClick={() => void cancelPull(model.id)} className="rounded-[4px] underline underline-offset-2 hover:text-text">
              Cancel
            </button>
          )}
        </span>
      </div>
      <Progress value={waiting ? 0 : t.fraction} label={`${model.name} download`} className={waiting ? undefined : 'dawn-bar'} />
    </div>
  );
}

/** The composer, pared down: one prompt, held until the model arrives. */
function FirstPrompt({ ready, model }: { ready: boolean; model?: Model }) {
  const prompt = useApp((s) => s.draft.prompt);
  const groups = useApp((s) => s.groups);
  const developing = useApp(isDeveloping);
  const waiting = groups.filter((g) => g.prints.some((p) => p.status === 'waiting'));
  return (
    <section className="flex flex-col gap-2">
      {waiting.map((g) => (
        <div key={g.id} className="flex items-center justify-between gap-4 rounded-card border border-line px-3.5 py-2.5">
          <div className="flex min-w-0 flex-col gap-0.5">
            <span className="truncate text-[14px]">{g.prompt}</span>
            <span className="edge text-muted">Waiting · develops when {model?.name ?? 'the model'} is ready</span>
          </div>
          <Button size="sm" variant="ghost" onClick={() => void stopGroup(g)}>
            Remove
          </Button>
        </div>
      ))}
      <div className={cx('flex flex-col gap-2.5 rounded-composer border bg-surface px-3.5 pt-3.5 pb-3', ready ? 'border-line-strong' : 'border-line')}>
        <label htmlFor="prompt" className="sr-only">
          Your first prompt
        </label>
        <textarea
          id="prompt"
          rows={2}
          value={prompt}
          placeholder="A lighthouse at first light, long exposure, film grain"
          onChange={(e) => setDraft({ prompt: e.target.value })}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && ready) {
              e.preventDefault();
              develop();
            }
          }}
          className="selectable w-full resize-none border-0 bg-transparent p-0 text-[15px] leading-normal text-text outline-0 placeholder:text-muted/60"
        />
        <div className="flex items-center gap-3">
          <span className="edge text-[11.5px] text-muted">{ready ? `Write your first prompt while this downloads. It develops when ${model?.name ?? 'the model'} is ready.` : 'You can write your first prompt now. Pick a starter to develop it.'}</span>
          <span className="flex-1" />
          <DevelopButton ready={ready && prompt.trim().length > 0} developing={developing && false} label="Develop when ready" />
        </div>
      </div>
    </section>
  );
}

/** No GPU the engine can use. Say so plainly, and what to try. */
export function NoGpu({ onRetry }: { onRetry: () => void }) {
  const steps = ['Install or update your graphics driver (NVIDIA, AMD or Intel), then restart your computer.', 'Come back and check again.'];
  return (
    <main className="flex min-h-0 flex-1 flex-col items-center overflow-y-auto px-7">
      <div className="enter flex w-full max-w-[600px] flex-1 flex-col justify-center gap-6 py-10">
        <header className="flex flex-col gap-3">
          <h1 className="m-0 font-display text-[44px] leading-[1.1] font-light tracking-[-0.02em]">No graphics card found.</h1>
          <p className="m-0 text-[15px] leading-normal text-muted">
            {PRODUCT_NAME} makes images on your own graphics card: NVIDIA through CUDA, AMD and Intel through Vulkan (experimental), including built-in graphics.
          </p>
        </header>
        <section className="flex flex-col gap-3 rounded-card border border-line bg-surface p-4">
          <span className="text-[13px] font-medium">If this computer has one</span>
          <ol className="m-0 flex list-none flex-col gap-2.5 p-0">
            {steps.map((text, i) => (
              <li key={text} className="flex gap-3 text-[13px] leading-snug text-muted">
                <span className="edge mt-px flex size-5 flex-none items-center justify-center rounded-full border border-line-strong text-text">{i + 1}</span>
                {text}
              </li>
            ))}
          </ol>
        </section>
        <div className="flex gap-2">
          <Button variant="solid" onClick={onRetry} className="h-9 rounded-card px-4 text-[14px]">
            Check again
          </Button>
          <Button onClick={() => openExternal('https://www.nvidia.com/en-us/drivers/')} className="border-line-strong">
            NVIDIA drivers
          </Button>
          <Button onClick={() => openExternal('https://www.amd.com/en/support/download/drivers.html')} className="border-line-strong">
            AMD drivers
          </Button>
          <Button onClick={() => openExternal('https://www.intel.com/content/www/us/en/support/detect.html')} className="border-line-strong">
            Intel drivers
          </Button>
        </div>
        <p className="m-0 text-[12.5px] text-muted">Mac support, and cloud models with your own keys, come after v0.1.</p>
      </div>
    </main>
  );
}

/** The engine isn't answering: say what Studio did about it and what the user can do. */
export function EngineDown({ error, onRetry }: { error: string | null; onRetry: () => void }) {
  return (
    <Splash title="The engine isn't running.">
      <p className="m-0 max-w-[520px] text-[15px] leading-normal text-muted">
        {error ?? (inTauri ? "Studio tried to start it, but it didn't answer." : 'Start it in a terminal, and this screen will pick it up by itself.')}
      </p>
      {!inTauri && <pre className="selectable m-0 rounded-control border border-line bg-surface px-3 py-2 font-mono text-[12.5px]">dawnroom serve</pre>}
      {inTauri && (
        <Button variant="solid" onClick={onRetry} className="h-9 rounded-card px-4 text-[14px]">
          Try again
        </Button>
      )}
    </Splash>
  );
}

/** The engine answers but predates this Studio. Say which one, and how to bring it up to date. */
export function EngineOutdated() {
  const version = useApp((s) => s.health?.version);
  return (
    <Splash title="The engine is older than Studio.">
      <p className="m-0 max-w-[560px] text-[15px] leading-normal text-muted">
        The engine at {ENGINE_URL.replace('http://', '')}
        {version ? ` (${version})` : ''} is missing things this Studio needs. Stop it, update or rebuild it, and start it again. This screen picks it up by itself.
      </p>
      <pre className="selectable m-0 rounded-control border border-line bg-surface px-3 py-2 text-left font-mono text-[12.5px]">cargo build --manifest-path engine/Cargo.toml</pre>
    </Splash>
  );
}
