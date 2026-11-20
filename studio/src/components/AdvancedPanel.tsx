// Everything a darkroom regular might want and a newcomer never needs: seed, steps,
// guidance, negative prompt, custom size. Docked beside the stream, hidden by default.

import type { ReactNode } from 'react';
import { emptyAdvanced, modelFor, setDraft, toggleAdvanced, useApp } from '../app/state';
import { Button, cx, IconButton } from './ui';

const input = 'selectable h-9 w-full min-w-0 rounded-control border border-line bg-bg px-2.5 font-mono text-[12.5px] text-text outline-0 placeholder:text-muted/70 focus-visible:border-line-strong focus-visible:outline-2';

function Field({ label, hint, htmlFor, children }: { label: string; hint?: ReactNode; htmlFor: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <label htmlFor={htmlFor} className="text-[12px] font-medium text-muted">
        {label}
      </label>
      {children}
      {hint && <p className="m-0 text-[11.5px] leading-snug text-muted">{hint}</p>}
    </div>
  );
}

export function AdvancedPanel() {
  const d = useApp((s) => s.draft);
  const model = useApp((s) => modelFor({ model: s.draft.models[s.draft.task] ?? undefined, task: s.draft.task }, s));
  const guidance = d.cfg.trim() ? Number(d.cfg) : (model?.defaults.cfg ?? 1);

  return (
    <aside aria-label="Advanced settings" className="enter flex w-[300px] flex-none flex-col border-l border-line">
      <div className="flex h-[52px] flex-none items-center justify-between pr-3 pl-5">
        <h2 className="m-0 text-[14px] font-semibold">Advanced</h2>
        <IconButton icon="close" label="Close Advanced" size={30} onClick={toggleAdvanced} className="border-transparent" />
      </div>
      <div className="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-5 pb-5">
        <Field label="Seed" htmlFor="adv-seed" hint={d.seedLocked ? 'Locked: every image starts from this seed, so only the prompt changes the result.' : 'Random each time. Lock a seed to change one thing at a time.'}>
          <div className="flex gap-1.5">
            <input id="adv-seed" inputMode="numeric" value={d.seed} placeholder="random" onChange={(e) => setDraft({ seed: e.target.value.replace(/\D/g, ''), seedLocked: e.target.value.trim() !== '' })} className={input} />
            <IconButton icon="dice" label="Pick a random seed and lock it" size={36} onClick={() => setDraft({ seed: String(Math.floor(Math.random() * 2 ** 32)), seedLocked: true })} />
            <IconButton
              icon={d.seedLocked ? 'lock' : 'unlock'}
              label={d.seedLocked ? 'Unlock the seed' : 'Lock the seed'}
              aria-pressed={d.seedLocked}
              disabled={!d.seed}
              size={36}
              onClick={() => setDraft({ seedLocked: !d.seedLocked })}
              className={d.seedLocked ? 'border-line-strong bg-raised text-text' : ''}
            />
          </div>
        </Field>

        <div className="grid grid-cols-2 gap-3">
          <Field label="Steps" htmlFor="adv-steps">
            <input id="adv-steps" type="number" min={1} max={50} value={d.steps} placeholder={String(model?.defaults.steps ?? '')} onChange={(e) => setDraft({ steps: e.target.value })} className={input} />
          </Field>
          <Field label="Guidance" htmlFor="adv-cfg">
            <input id="adv-cfg" type="number" min={0} max={20} step={0.5} value={d.cfg} placeholder={String(model?.defaults.cfg ?? '')} onChange={(e) => setDraft({ cfg: e.target.value })} className={input} />
          </Field>
          <p className="col-span-2 -mt-1.5 m-0 text-[11.5px] leading-snug text-muted">{model ? `${model.name} is tuned for ${model.defaults.steps} steps at guidance ${model.defaults.cfg}. Leave these empty to use that.` : 'Leave these empty to use the model’s own.'}</p>
        </div>

        <Field label="Negative prompt" htmlFor="adv-neg" hint={guidance <= 1 ? 'At guidance 1 the model ignores this. Raise guidance to use it.' : undefined}>
          <textarea id="adv-neg" rows={3} value={d.negative} placeholder="What to keep out of the image" onChange={(e) => setDraft({ negative: e.target.value })} className={cx(input, 'h-auto resize-none py-2 font-sans text-[13px] leading-snug')} />
        </Field>

        <div className="flex flex-col gap-2">
          <label className="flex items-center gap-2 text-[12px] font-medium text-muted">
            <input type="checkbox" checked={d.customSize} onChange={(e) => setDraft({ customSize: e.target.checked })} className="size-3.5 accent-(--color-text)" />
            Custom size
          </label>
          {d.customSize && (
            <>
              <div className="flex items-center gap-2">
                <input aria-label="Width in pixels" type="number" min={256} max={2048} step={8} value={d.width} onChange={(e) => setDraft({ width: e.target.value })} className={input} />
                <span className="text-muted">×</span>
                <input aria-label="Height in pixels" type="number" min={256} max={2048} step={8} value={d.height} onChange={(e) => setDraft({ height: e.target.value })} className={input} />
              </div>
              <p className="m-0 text-[11.5px] leading-snug text-muted">Multiples of 8, from 256 to 2048. The models were measured near 1024 × 1024; much bigger may be slow or run out of memory.</p>
            </>
          )}
        </div>

        <div className="flex-1" />
        <div className="flex items-center justify-between gap-3 border-t border-line pt-4">
          <span className="text-[12px] text-muted">Applies to the next image.</span>
          <Button size="sm" variant="ghost" onClick={() => setDraft(emptyAdvanced)}>
            Reset
          </Button>
        </div>
      </div>
    </aside>
  );
}
