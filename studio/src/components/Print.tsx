// One image in the Create stream: waiting, developing, done or failed (a print that didn't
// happen collapses to a strip, so it doesn't hold a whole frame's space). Every finished print
// carries its recipe as a film-edge caption.

import { useEffect, useRef, useState, type CSSProperties } from 'react';
import { api, editThis, go, retry, sameSeed, useApp, vary, view, type Group, type Print } from '../app/state';
import { filmEdge, recipeText, stageLabel, stageProgress } from '../lib/format';
import { Grain } from './Moments';
import { Button, cx, Icon, IconButton, useCopy } from './ui';

/** Whole seconds since `since`, ticking while mounted. */
function useElapsed(since: number | null): number {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (since == null) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [since]);
  return since == null ? 0 : Math.max(0, Math.round((now - since) / 1000));
}

export function PrintFrame({ print, group, label }: { print: Print; group: Group; label?: string }) {
  const s = print.status;
  // Did this print develop while we watched? Then it gets the lights-on moment; one loaded from history doesn't.
  const watched = useRef(false);
  if (s === 'running') watched.current = true;
  return (
    <figure className="@container m-0 flex min-w-0 flex-col gap-2">
      <div className={cx('relative overflow-hidden rounded-frame', s === 'completed' ? 'shadow-[0_0_0_1px_var(--color-line),var(--shadow-print)]' : 'shadow-[0_0_0_1px_var(--color-line)]')} style={s === 'failed' || s === 'cancelled' ? { height: 88 } : { aspectRatio: `${group.width} / ${group.height}` }}>
        {s === 'completed' && print.job ? <Finished print={print} fresh={watched.current} /> : s === 'running' ? <Developing print={print} /> : s === 'failed' ? <Failed /> : <Blank />}
        {label && s === 'completed' && <span className="edge absolute top-2.5 left-2.5 rounded-[4px] bg-bg/75 px-1.5 py-0.5 text-text">{label}</span>}
      </div>
      {s === 'completed' && print.job ? <Done print={print} group={group} /> : s === 'running' ? <RunningCaption print={print} /> : s === 'failed' ? <FailedCaption print={print} /> : <WaitingCaption print={print} group={group} />}
    </figure>
  );
}

/** An unexposed frame: surface, with a little grain so it reads as film rather than a hole. */
function Blank() {
  return (
    <div className="absolute inset-0 bg-surface">
      <Grain strength={0.2} />
    </div>
  );
}

function Failed() {
  return (
    <div className="absolute inset-0 flex items-center justify-center bg-bg text-danger">
      <Icon name="warn" size={22} />
    </div>
  );
}

/**
 * The finished print. When it has just developed (`fresh`), the last preview stays underneath
 * until the full image has loaded; then the lights come on for 120 ms and the print settles.
 */
function Finished({ print, fresh }: { print: Print; fresh: boolean }) {
  const job = print.job!;
  const [loaded, setLoaded] = useState(false);
  return (
    <>
      {fresh && !loaded && print.preview && <img src={print.preview} alt="" draggable={false} className="absolute inset-0 size-full object-cover" />}
      <img src={api.imageUrl(job.id)} alt={job.prompt} draggable={false} onLoad={() => setLoaded(true)} className={cx('absolute inset-0 size-full object-cover', fresh && (loaded ? 'settle' : 'opacity-0'))} />
      {fresh && loaded && <div className="lights-on" />}
    </>
  );
}

/**
 * The developing ritual. The real step preview starts as a safelight-tinted monochrome under
 * heavy grain; colour and sharpness come in step by step (--p runs 0 → 1). Turbo models look
 * near-final at step 1, so the ramp is what makes the image feel like it resolves.
 */
function Developing({ print }: { print: Print }) {
  const p = print.progress;
  const fraction = stageProgress(p);
  return (
    <div role="img" aria-label={stageLabel(p)} className="developing absolute inset-0" style={{ '--p': fraction } as CSSProperties}>
      {print.preview && <img src={print.preview} alt="" draggable={false} className="preview" />}
      <div className="tint" />
      <Grain />
      {p?.stage === 'sampling' && p.step != null && (
        <div className="edge absolute top-2.5 left-2.5 rounded-[4px] bg-bg/70 px-1.5 py-0.5 text-text">
          {p.step}/{p.total}
        </div>
      )}
      <div className="absolute inset-x-0 bottom-0 h-[3px] bg-text/10">
        <div className="h-full bg-safelight transition-[width] duration-500" style={{ width: `${fraction * 100}%` }} />
      </div>
    </div>
  );
}

function RunningCaption({ print }: { print: Print }) {
  const elapsed = useElapsed(print.runningSince);
  return (
    <figcaption className="edge flex justify-between gap-3" aria-live="polite">
      <span>{print.stopping ? 'Stopping…' : stageLabel(print.progress)}</span>
      <span className="text-muted">{elapsed} s</span>
    </figcaption>
  );
}

function WaitingCaption({ print, group }: { print: Print; group: Group }) {
  const model = useApp((s) => s.models.find((m) => m.id === group.waitingFor)?.name);
  const othersBusy = useApp((s) => !!s.health?.busy && !s.groups.some((g) => g.prints.some((p) => p.status === 'running')));
  const text =
    print.status === 'cancelled'
      ? ['Stopped', '']
      : print.status === 'waiting'
        ? ['Waiting', model ? `develops when ${model} has downloaded` : 'develops when the model has downloaded']
        : [print.stopping ? 'Stopping…' : 'Next up', print.stopping ? '' : othersBusy ? 'the GPU is finishing another job' : 'starts when the one before is done'];
  return (
    <figcaption className="edge flex items-center justify-between gap-3 text-muted">
      <span>{text[0]}</span>
      {print.status === 'cancelled' ? (
        <Button size="sm" variant="ghost" icon="retry" onClick={() => retry(print)} className="-my-1.5">
          Develop again
        </Button>
      ) : (
        <span className="truncate">{text[1]}</span>
      )}
    </figcaption>
  );
}

function FailedCaption({ print }: { print: Print }) {
  return (
    <figcaption className="flex flex-col gap-2">
      <span className="edge text-danger">Didn't develop</span>
      <span role="alert" className="selectable text-[12.5px] leading-snug text-text">
        {print.error}
      </span>
      <div className="flex gap-1.5">
        {print.errorCode === 'model_not_installed' && (
          <Button size="sm" icon="models" onClick={() => go('models')}>
            Open Models
          </Button>
        )}
        <Button size="sm" icon="retry" onClick={() => retry(print)}>
          Try again
        </Button>
      </div>
    </figcaption>
  );
}

/** The film-edge caption and what you can do with a finished print. */
function Done({ print, group }: { print: Print; group: Group }) {
  const job = print.job!;
  const [copied, copy] = useCopy();
  // Labels show when the print is wide enough for them; narrow prints keep the icons (and tooltips).
  const text = 'hidden @[400px]:inline';
  return (
    <>
      <figcaption className="edge selectable truncate text-muted">{filmEdge(job)}</figcaption>
      <div className="flex gap-1.5">
        <Button size="sm" icon="vary" aria-label="Vary" title="Vary: same prompt, new seed" onClick={() => vary(print, group)}>
          <span className={text}>Vary</span>
        </Button>
        <Button size="sm" icon="lock" aria-label="Same seed" title="Same seed: keep this seed for the next image" onClick={() => sameSeed(print)}>
          <span className={text}>Same seed</span>
        </Button>
        <Button size="sm" icon={copied ? 'check' : 'copy'} aria-label="Copy recipe" title="Copy recipe" onClick={() => copy(recipeText(job))}>
          <span className={text}>{copied ? 'Copied' : 'Copy recipe'}</span>
        </Button>
        <Button size="sm" icon="edit" aria-label="Edit this" title="Edit this: start an edit from this print" onClick={() => editThis(job)}>
          <span className={text}>Edit this</span>
        </Button>
        <IconButton size={30} icon="open" label="Open in Library" onClick={() => view(job.id)} className="ml-auto" />
      </div>
    </>
  );
}
