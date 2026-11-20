// The one composer: attachments, prompt, Generate | Edit, the model switcher (Auto first), shape,
// count and Develop. Everything else lives in the Advanced panel.

import { DOCS } from '../config';
import { HowLink } from './ui';
import { useEffect, useRef } from 'react';
import { attachFile, develop, isDeveloping, modelFor, pullModels, setDraft, sizeFor, store, toggleAdvanced, useApp } from '../app/state';
import type { Aspect } from '../api/types';
import { bytes } from '../lib/format';
import { TASKS } from '../lib/models';
import { AttachButton, Attachments, imageFrom } from './InputWells';
import { AutoReasonLine, ModelSwitcher, modelFacts } from './ModelSwitcher';
import { cx, IconButton, Segmented } from './ui';

/** A little outline of the frame shape, like the canvas's aspect chips. */
function AspectGlyph({ a }: { a: Aspect }) {
  const long = 14;
  const [w, h] = a.width >= a.height ? [long, Math.round((long * a.height) / a.width)] : [Math.round((long * a.width) / a.height), long];
  return <span className="block border-[1.5px] border-current" style={{ width: w, height: h }} />;
}

export function Composer({ placeholder }: { placeholder?: string }) {
  const draft = useApp((s) => s.draft);
  const aspects = useApp((s) => s.aspects);
  const advancedOpen = useApp((s) => s.advancedOpen);
  const focusTick = useApp((s) => s.focusPrompt);
  const developing = useApp(isDeveloping);
  const text = useRef<HTMLTextAreaElement>(null);
  const edit = draft.task === 'edit';

  // Grow with the prompt, up to six lines.
  useEffect(() => {
    const el = text.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, 6 * 22.5)}px`;
  }, [draft.prompt]);

  useEffect(() => {
    text.current?.focus();
  }, [focusTick]);

  const ready = draft.prompt.trim().length > 0 && (!edit || draft.image?.status === 'ready');
  const custom = draft.customSize || draft.steps !== '' || draft.cfg !== '' || draft.negative !== '' || draft.seedLocked;
  const hint = placeholder ?? (edit ? (draft.image ? 'Say what to change: "make it night", "put the hat from image 2 on the cat"…' : 'Attach the image to edit (drop, paste or +), then say what to change…') : 'Describe what you want to see…');
  const shape = edit ? draft.editAspect : draft.aspect;

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-col gap-3 rounded-composer border border-line-strong bg-surface px-3.5 pt-3.5 pb-2.5 focus-within:border-text/25">
        <Attachments />
        <label htmlFor="prompt" className="sr-only">
          {edit ? 'Instruction' : 'Prompt'}
        </label>
        <textarea
          id="prompt"
          ref={text}
          rows={2}
          value={draft.prompt}
          placeholder={developing ? 'Next idea? You can type while this develops.' : hint}
          onChange={(e) => setDraft({ prompt: e.target.value })}
          onPaste={(e) => {
            // A pasted image is an attachment, like in a chat; text pastes as usual.
            const f = imageFrom(e.clipboardData.items);
            if (!f) return;
            e.preventDefault();
            attachFile(f);
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
              e.preventDefault();
              develop();
            }
          }}
          className="selectable w-full resize-none border-0 bg-transparent p-0 text-[15px] leading-normal text-text outline-0 placeholder:text-muted"
        />
        {/* Wraps when the Advanced panel squeezes it: the controls stay on one line, Develop drops to the next. */}
        <div className="flex flex-wrap items-center gap-2">
          <AttachButton />
          <Segmented label="Task" value={draft.task} onChange={(task) => setDraft({ task })} options={TASKS.map((t) => ({ id: t.id, label: t.label }))} />
          <ModelSwitcher />
          <div role="radiogroup" aria-label="Shape" className="flex gap-1">
            {edit && (
              <button
                type="button"
                role="radio"
                aria-checked={!draft.customSize && shape === 'original'}
                title="Keep the shape of the image you edit"
                onClick={() => setDraft({ editAspect: 'original', customSize: false })}
                className={cx('edge flex h-8 items-center rounded-control border px-2 text-[12px] transition-colors', !draft.customSize && shape === 'original' ? 'border-line-strong bg-raised text-text' : 'border-line text-muted hover:bg-raised hover:text-text')}
              >
                Original
              </button>
            )}
            {aspects.map((a) => {
              const on = !draft.customSize && a.id === shape;
              return (
                <button
                  key={a.id}
                  type="button"
                  role="radio"
                  aria-checked={on}
                  aria-label={`${a.id}, ${a.width} by ${a.height}`}
                  title={`${a.id} · ${a.width} × ${a.height}`}
                  onClick={() => setDraft(edit ? { editAspect: a.id, customSize: false } : { aspect: a.id, customSize: false })}
                  className={cx('edge flex h-8 items-center gap-1.5 rounded-control border px-2 text-[12px] transition-colors', on ? 'border-line-strong bg-raised text-text' : 'border-line text-muted hover:bg-raised hover:text-text')}
                >
                  <AspectGlyph a={a} />
                  {/* Only the chosen shape spells out its ratio, so all of them fit. */}
                  {on && a.id}
                </button>
              );
            })}
          </div>
          <div className="flex h-8 flex-none items-center rounded-control border border-line">
            <button type="button" aria-label="Fewer images" disabled={draft.count <= 1} onClick={() => setDraft({ count: Math.max(1, store.get().draft.count - 1) })} className="h-full w-[26px] rounded-l-[7px] text-muted hover:bg-raised hover:text-text disabled:opacity-40">
              −
            </button>
            <span className="edge min-w-[22px] text-center text-[12px]" aria-live="polite" aria-label={`${draft.count} ${draft.count === 1 ? 'image' : 'images'}`}>
              ×{draft.count}
            </span>
            <button type="button" aria-label="More images" disabled={draft.count >= 4} onClick={() => setDraft({ count: Math.min(4, store.get().draft.count + 1) })} className="h-full w-[26px] rounded-r-[7px] text-muted hover:bg-raised hover:text-text disabled:opacity-40">
              +
            </button>
          </div>
          <div className="flex flex-1 items-center justify-end gap-2">
            <div className="relative">
              <IconButton icon="settings" label="Advanced" aria-expanded={advancedOpen} onClick={toggleAdvanced} className={advancedOpen ? 'border-line-strong bg-raised text-text' : ''} />
              {custom && !advancedOpen && <span className="absolute -top-0.5 -right-0.5 size-1.5 rounded-full bg-text" title="Advanced settings are changed" />}
            </div>
            <DevelopButton ready={ready} developing={developing} />
          </div>
        </div>
      </div>
      <Hint />
    </div>
  );
}

/** The only filled accent in the app. */
export function DevelopButton({ ready, developing, label = 'Develop' }: { ready: boolean; developing: boolean; label?: string }) {
  if (!ready && developing)
    return (
      <button type="button" disabled className="flex h-9 flex-none items-center gap-2 rounded-card border border-line-strong bg-raised px-4 text-[14px] font-medium text-text">
        <span className="breathe size-1.5 rounded-full bg-safelight-to" />
        Developing…
      </button>
    );
  return (
    <button
      type="button"
      disabled={!ready}
      onClick={develop}
      className={cx('flex h-9 flex-none items-center gap-2.5 rounded-card px-4 text-[14px] font-semibold whitespace-nowrap transition-[filter]', ready ? 'bg-safelight text-bg hover:brightness-110' : 'border border-line bg-surface text-muted')}
    >
      {label}
      <span className="edge text-[10.5px] font-normal opacity-70">Ctrl ↵</span>
    </button>
  );
}

/** One quiet line under the composer: Auto's reason (or the chosen model's facts) and the size, or what's in the way. */
function Hint() {
  const s = useApp((x) => x);
  const d = s.draft;
  const edit = d.task === 'edit';
  const chosen = d.models[d.task];
  const aspect = d.customSize ? undefined : edit ? (d.editAspect === 'original' ? undefined : d.editAspect) : d.aspect;
  const req = { model: chosen ?? undefined, task: d.task, aspect, width: d.customSize ? Number(d.width) : undefined, height: d.customSize ? Number(d.height) : undefined };
  const model = modelFor(req, s);
  const line = 'edge m-0 text-center text-muted';
  // Auto has no pick (nothing installed for this task): its own message, with the download it suggests.
  if (!model) return <p className={line}>{chosen ? ' ' : <AutoReasonLine />}</p>;
  if (model.state !== 'installed') {
    const coming = model.state === 'pulling' || model.state === 'verifying' || s.pullQueue.includes(model.id);
    if (coming) return <p className={line}>{model.name} is downloading. Your image develops when it arrives.</p>;
    return (
      <p className={line}>
        {model.name} isn't on this machine yet.{' '}
        <button type="button" onClick={() => pullModels([model.id])} className="rounded-[4px] text-link underline underline-offset-2">
          Download · {bytes(model.download_bytes || model.size_bytes)}
        </button>
      </p>
    );
  }
  const { width, height } = sizeFor(req, s, edit ? d.image : null);
  // Other apps holding the VRAM is the usual reason a run crawls or fails; say so before it does.
  const peak = model.measured.find((m) => m.gpu === s.gpu?.name)?.peak_vram_mib;
  const live = s.health?.gpu;
  if (peak && live && s.health?.backend === 'stopped' && live.vram_total_mib - live.vram_used_mib < peak) {
    return (
      <p className="edge m-0 text-center text-text">
        Other apps are using {(live.vram_used_mib / 1024).toFixed(1)} of your GPU's {Math.round(live.vram_total_mib / 1024)} GB. {model.name} needs about {(peak / 1024).toFixed(1)} GB: close them, or expect a slow run.
      </p>
    );
  }
  const shape = edit && !req.aspect && !req.width ? (d.image ? `about ${width} × ${height}` : "the image's shape") : `${width} × ${height}`;
  const facts = chosen ? [model.name, ...modelFacts(model, d.task).slice(1)] : null;
  return (
    <p className={line}>
      {facts ? facts.join(' · ') : <AutoReasonLine />}
      {` · ${shape}`}
      {d.seedLocked && d.seed ? ` · seed ${d.seed} locked` : ''}
      {' · '}
      <HowLink href={edit ? DOCS.edit : chosen ? DOCS.models : DOCS.auto} className="font-sans text-[11.5px]" />
    </p>
  );
}
