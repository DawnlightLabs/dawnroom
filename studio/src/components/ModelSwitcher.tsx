// The model switcher: Auto first, then the installed models that can do the composer's task. Each row
// says what the engine knows: size, memory (GPU, RAM offload, estimate) and the time on this machine, if we have one.
// Auto's one-line reason stays visible on the line under the composer.

import { useEffect, useRef, useState } from 'react';
import type { Model, Task } from '../api/types';
import { autoPick, go, pullModels, setDraft, store, useApp } from '../app/state';
import { bytes } from '../lib/format';
import { autoReason, fitFor, memoryLabel, runsHere, switcherOptions, tagLine, timingFor, timingLabel } from '../lib/models';
import { cx, Icon } from './ui';

/** The one label a model can carry: it was added by the user, not from Dawnroom's library. */
export function CustomBadge() {
  return <span className="edge flex-none rounded-[3px] border border-ring px-1 text-[10.5px] leading-[16px] text-text">Custom</span>;
}

/** One line of facts for a model on a task: "q4 · 6.7 GB · 4.3 GB on the GPU · about 19 s". */
export function modelFacts(m: Model, task: Task): string[] {
  return [tagLine(m), memoryLabel(fitFor(m, task)), timingLabel(timingFor(m, task))].filter(Boolean);
}

export function ModelSwitcher() {
  const task = useApp((s) => s.draft.task);
  const chosen = useApp((s) => s.draft.models[s.draft.task]);
  const models = useApp((s) => s.models);
  const pick = useApp(autoPick);
  const [open, setOpen] = useState(false);
  const button = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const options = switcherOptions(models, task);
  const model = chosen ? models.find((m) => m.id === chosen) : undefined;
  const autoModel = pick?.ok ? models.find((m) => m.id === pick.model) : undefined;

  // A chosen model that went away (deleted, or this tab can't use it): back to Auto.
  useEffect(() => {
    if (chosen && !options.some((m) => m.id === chosen)) setDraft({ models: { ...store.get().draft.models, [task]: null } });
  }, [chosen, options, task]);

  useEffect(() => {
    if (!open) return;
    list.current?.querySelector<HTMLElement>('[aria-checked="true"]')?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        setOpen(false);
        button.current?.focus();
      }
    };
    addEventListener('keydown', onKey);
    return () => removeEventListener('keydown', onKey);
  }, [open]);

  const choose = (id: string | null) => {
    setDraft({ models: { ...store.get().draft.models, [task]: id } });
    setOpen(false);
    button.current?.focus();
  };

  // Up/down walk the rows (a radio group), like a native list.
  const onListKey = (e: React.KeyboardEvent) => {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    e.preventDefault();
    const rows = [...(list.current?.querySelectorAll<HTMLElement>('[role="radio"]:not([disabled])') ?? [])];
    const i = rows.indexOf(document.activeElement as HTMLElement);
    rows[(i + (e.key === 'ArrowDown' ? 1 : -1) + rows.length) % rows.length]?.focus();
  };

  const shown = model ?? autoModel;

  return (
    <div className="relative">
      <button
        ref={button}
        type="button"
        aria-haspopup="true"
        aria-expanded={open}
        aria-label={`Model: ${model ? model.name : `Auto${autoModel ? `, ${autoModel.name}` : ''}`}. Change`}
        title={shown ? modelFacts(shown, task).join(' · ') : undefined}
        onClick={() => setOpen((o) => !o)}
        className={cx('flex h-8 max-w-[280px] items-center gap-1.5 rounded-control border px-2.5 text-[12.5px] transition-colors', open ? 'border-line-strong bg-raised text-text' : 'border-line text-text hover:bg-raised')}
      >
        {model ? (
          <span className="truncate font-medium">{model.name}</span>
        ) : (
          <>
            <span className="font-medium">Auto</span>
            {autoModel && <span className="truncate text-muted">· {autoModel.name}</span>}
          </>
        )}
        {shown?.tag && <span className="edge flex-none text-muted">{shown.tag}</span>}
        <span className="flex-none text-muted"><Icon name="chevron" size={13} /></span>
      </button>

      {open && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setOpen(false)} aria-hidden="true" />
          <div
            ref={list}
            role="radiogroup"
            aria-label={`Model for ${task === 'edit' ? 'editing' : 'making images'}`}
            onKeyDown={onListKey}
            className="enter absolute bottom-[calc(100%+8px)] left-0 z-50 flex max-h-[min(520px,calc(100vh-200px))] w-[440px] flex-col overflow-y-auto rounded-card border border-line-strong bg-surface p-1.5 shadow-[0_18px_48px_rgb(0_0_0/0.35)]"
          >
            <Row checked={!chosen} onClick={() => choose(null)} title="Auto" note={pick ? (pick.ok ? `Picks ${pick.name} now. ${autoReason(pick)}.` : autoReason(pick)) : 'Picks the best model you have that runs on this machine.'} />
            <div className="mx-2 my-1.5 h-px bg-line" />
            {options.length === 0 && <p className="m-0 px-3 py-2 text-[12.5px] text-muted">No installed model can {task === 'edit' ? 'edit images' : 'make images'} yet.</p>}
            {options.map((m) => {
              const fit = fitFor(m, task);
              return (
                <Row
                  key={m.id}
                  checked={chosen === m.id}
                  disabled={!runsHere(fit)}
                  onClick={() => choose(m.id)}
                  title={m.name}
                  tag={m.tag}
                  custom={!!m.custom}
                  facts={modelFacts(m, task)}
                  warn={fit.tier === 'offload'}
                  note={m.custom ? (m.label ?? 'Custom, not reviewed by Dawnroom') : undefined}
                />
              );
            })}
            <div className="mx-2 my-1.5 h-px bg-line" />
            <button type="button" onClick={() => go('models')} className="flex items-center justify-between rounded-control px-3 py-2 text-left text-[12.5px] text-muted hover:bg-raised hover:text-text">
              Browse all models
              <Icon name="next" size={14} />
            </button>
          </div>
        </>
      )}
    </div>
  );
}

/** Auto's one-line reason (the engine's words), or why Auto has nothing and the way out. */
export function AutoReasonLine() {
  const pick = useApp(autoPick);
  const models = useApp((s) => s.models);
  const pulls = useApp((s) => s.pulls);
  if (!pick) return null;
  const suggest = pick.ok ? null : pick.suggest;
  const pulling = suggest && (suggest.model in pulls || models.find((m) => m.id === suggest.model)?.state === 'pulling');
  return (
    <span className={pick.ok ? undefined : 'text-text'}>
      {pick.ok ? `Auto picked ${pick.name}: ${autoReason(pick).replace(/^./, (c) => c.toLowerCase())}` : autoReason(pick)}
      {suggest && (
        <>
          {' '}
          <button type="button" disabled={!!pulling} onClick={() => pullModels([suggest.model])} className="rounded-[4px] text-link underline underline-offset-2 disabled:no-underline disabled:opacity-70">
            {pulling ? `Downloading ${suggest.name}…` : `Get ${suggest.name} · ${bytes(suggest.download_bytes)}`}
          </button>
        </>
      )}
    </span>
  );
}

function Row({ checked, disabled, onClick, title, tag, custom, facts, note, warn }: { checked: boolean; disabled?: boolean; onClick: () => void; title: string; tag?: string | null; custom?: boolean; facts?: string[]; note?: string; warn?: boolean }) {
  return (
    <button type="button" role="radio" aria-checked={checked} disabled={disabled} onClick={onClick} className={cx('flex items-start gap-2.5 rounded-control px-3 py-2 text-left transition-colors disabled:opacity-50', checked ? 'bg-raised' : 'hover:bg-raised')}>
      <span className={cx('mt-[5px] flex size-[11px] flex-none items-center justify-center rounded-full border-[1.5px]', checked ? 'border-text' : 'border-muted')}>{checked && <span className="size-[5px] rounded-full bg-text" />}</span>
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="flex items-baseline gap-1.5">
          <span className="truncate text-[13.5px] font-medium text-text">{title}</span>
          {tag && <span className="edge text-muted">{tag}</span>}
          <span className="flex-1" />
          {custom && <CustomBadge />}
        </span>
        {facts && (
          <span className="edge flex flex-wrap gap-x-1.5 text-muted">
            {facts.map((f, i) => (
              <span key={i} className={cx(i === 1 && warn && 'text-text')}>
                {i > 0 && '· '}
                {f}
              </span>
            ))}
          </span>
        )}
        {note && <span className="text-[12px] leading-snug text-muted">{note}</span>}
      </span>
    </button>
  );
}
