// Create: a vertical stream of prompt → prints, with the composer at the foot. Prints use the
// width the window has; an edit shows its original beside the result. Drop an image anywhere to edit it.

import { onDesktopDrop } from '../lib/tauri';
import { FirstTips } from '../components/FirstTips';
import { useEffect, useRef, useState } from 'react';
import { api, attachFile, attachPaths, dismissNotice, go, isDeveloping, retry, setDraft, stopGroup, store, useApp, type Group, type InputImage } from '../app/state';
import { AdvancedPanel } from '../components/AdvancedPanel';
import { Composer } from '../components/Composer';
import { PrintFrame } from '../components/Print';
import { Button, Icon, Mark, Notice } from '../components/ui';
import { duration } from '../lib/format';

// Each example shows off something different, and sets the preset and shape that suit it.
const EXAMPLES = [
  { prompt: 'A lighthouse at first light, long exposure, film grain', aspect: '1:1', note: '' },
  { prompt: 'Ceramic mug on linen, soft window light, 85mm product photo', aspect: '3:4', note: '' },
  { prompt: 'A hand-painted shop sign that reads OPEN LATE, wet street at night', aspect: '16:9', note: 'text works' },
];

export function Create() {
  const groups = useApp((s) => s.groups);
  const advancedOpen = useApp((s) => s.advancedOpen);
  const notice = useApp((s) => s.notice);
  const developing = useApp(isDeveloping);
  const scroller = useRef<HTMLDivElement>(null);
  // Dropping an image anywhere on Create attaches it, like a chat (the first becomes the image to edit).
  const [dragging, setDragging] = useState(false);
  // The desktop app hands over dropped files as paths (its own drag and drop); the browser uses the HTML drop below.
  useEffect(() => onDesktopDrop({ over: setDragging, drop: attachPaths }), []);
  const drag = useRef(0);

  // Stay pinned to the newest print as the stream grows, unless the user has scrolled up to look at older ones.
  const content = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  const lastTop = useRef(0);
  useEffect(() => {
    const box = scroller.current;
    if (!box || !content.current) return;
    const observer = new ResizeObserver(() => pinned.current && box.scrollTo({ top: box.scrollHeight }));
    observer.observe(content.current);
    observer.observe(box);
    return () => observer.disconnect();
  }, []);
  // A new group always comes into view.
  useEffect(() => {
    pinned.current = true;
    scroller.current?.scrollTo({ top: scroller.current.scrollHeight });
  }, [groups.length]);

  return (
    <div className="flex min-h-0 min-w-0 flex-1">
      <main
        className="relative flex min-w-0 flex-1 flex-col"
        onDragEnter={(e) => {
          if (![...e.dataTransfer.types].includes('Files')) return;
          drag.current++;
          setDragging(true);
        }}
        onDragOver={(e) => [...e.dataTransfer.types].includes('Files') && e.preventDefault()}
        onDragLeave={() => {
          drag.current = Math.max(0, drag.current - 1);
          if (!drag.current) setDragging(false);
        }}
        onDrop={(e) => {
          e.preventDefault();
          drag.current = 0;
          setDragging(false);
          Array.from(e.dataTransfer.files)
            .filter((f) => f.type.startsWith('image/'))
            .forEach(attachFile);
        }}
      >
        {/* Light's safelight: a thin lamp line along the top while anything develops (Dark lights the window edges). */}
        <div className="lamp-line z-10" data-on={developing} aria-hidden="true" />
        <div className="relative min-h-0 flex-1">
          <div
            ref={scroller}
            onScroll={(e) => {
              // Only the user scrolling up lets go of the bottom; the stream growing under us never does.
              const el = e.currentTarget;
              if (el.scrollHeight - el.scrollTop - el.clientHeight < 80) pinned.current = true;
              else if (el.scrollTop < lastTop.current - 2) pinned.current = false;
              lastTop.current = el.scrollTop;
            }}
            className="absolute inset-0 overflow-x-hidden overflow-y-auto px-8"
          >
            <div ref={content} className="flex min-h-full w-full flex-col justify-end">
              {groups.length === 0 ? <Empty /> : groups.map((g, i) => <ResultGroup key={g.id} group={g} first={i === 0} />)}
            </div>
          </div>
          <div className="pointer-events-none absolute inset-x-0 top-0 h-6 bg-linear-to-b from-bg to-transparent" />
        </div>
        <div className="flex-none px-8 pb-5">
          <div className="flex w-full flex-col gap-2">
            <FirstTips />
            {notice && (
              <Notice
                onDismiss={dismissNotice}
                action={
                  notice.code === 'model_not_installed' ? (
                    <Button size="sm" onClick={() => go('models')}>
                      Open Models
                    </Button>
                  ) : undefined
                }
              >
                {notice.text}
              </Notice>
            )}
            <Composer />
          </div>
        </div>
        {dragging && (
          <div className="pointer-events-none absolute inset-3 z-30 flex items-center justify-center rounded-card border-2 border-dashed border-ring bg-bg/80">
            <p className="m-0 flex items-center gap-2.5 text-[15px] text-text">
              <Icon name="image" size={20} />
              Drop to edit this image
            </p>
          </div>
        )}
      </main>
      {advancedOpen && <AdvancedPanel />}
    </div>
  );
}

function Empty() {
  return (
    <div className="enter flex max-w-[760px] flex-1 flex-col justify-center gap-7 py-10">
      <div className="flex flex-col gap-2">
        <Mark size={40} className="mb-3" />
        <h1 className="m-0 font-display text-[52px] leading-[1.05] font-light tracking-[-0.02em]">What shall we develop?</h1>
        <p className="m-0 text-[15px] text-muted">Describe it in plain words, or drop in an image to edit. It's made on this GPU and stays on this machine.</p>
      </div>
      <div className="flex flex-col">
        <span className="eyebrow pb-2 text-muted">Try one</span>
        {EXAMPLES.map((ex) => (
          <button
            key={ex.prompt}
            type="button"
            onClick={() => {
              setDraft({ task: 'generate', prompt: ex.prompt, aspect: ex.aspect, customSize: false });
              store.set((s) => ({ ...s, focusPrompt: s.focusPrompt + 1 }));
            }}
            className="group flex items-baseline justify-between gap-4 border-t border-line py-3 text-left text-[15px] text-muted transition-colors last:border-b hover:text-text"
          >
            {ex.prompt}
            <span className="edge flex-none">{[ex.aspect, ex.note].filter(Boolean).join(' · ')}</span>
          </button>
        ))}
      </div>
    </div>
  );
}

function ResultGroup({ group, first }: { group: Group; first: boolean }) {
  const n = group.prints.length;
  const busy = group.prints.some((p) => ['sending', 'waiting', 'queued', 'running'].includes(p.status));
  const done = group.prints.filter((p) => p.status === 'completed' && p.job?.finished_ms);
  const tail = !busy && done.length ? `ready in ${duration(Math.max(...done.map((p) => p.job!.finished_ms!)) - group.createdAt)}` : null;

  const rejected = group.prints.every((p) => p.status === 'failed' && !p.jobId) ? group.prints[0] : null;

  // Prints use the width there is, side by side, each as tall as the room above the composer allows
  // (so the prompt and the actions stay in view). An edit puts its original first, at the same height.
  const original = group.task === 'edit' ? group.inputs[0] : undefined;
  const refs = group.task === 'edit' ? group.inputs.slice(1) : [];
  const col = (w: number, h: number) => `minmax(0, calc(max(240px, 100vh - 300px) * ${w / h}))`;
  const layout = { gridTemplateColumns: [...(original ? [col(original.width, original.height)] : []), ...group.prints.map(() => col(group.width, group.height))].join(' ') };

  return (
    <section className={`enter flex flex-col gap-3 pt-5 pb-4 ${first ? '' : 'border-t border-line'}`}>
      <div className="flex items-end justify-between gap-4">
        <div className="flex min-w-0 flex-col gap-1.5">
          <p className="selectable m-0 text-[15px] leading-snug">{group.prompt}</p>
          <p className="edge m-0 text-muted">{[group.task === 'edit' ? 'Edit' : null, group.summary, n > 1 ? `×${n}` : null, refs.length ? `${refs.length} ${refs.length === 1 ? 'reference' : 'references'}` : null, tail].filter(Boolean).join(' · ')}</p>
        </div>
        {busy && (
          <Button size="sm" variant="outline" icon="stop" onClick={() => void stopGroup(group)} className="flex-none text-muted">
            Stop
          </Button>
        )}
      </div>
      {rejected ? (
        // The engine turned the request down before anything ran: one plain message, not a row of empty frames.
        <Notice
          action={
            <div className="flex flex-none gap-1.5">
              {rejected.errorCode === 'model_not_installed' && (
                <Button size="sm" icon="models" onClick={() => go('models')}>
                  Open Models
                </Button>
              )}
              <Button size="sm" icon="retry" onClick={() => group.prints.forEach(retry)}>
                Try again
              </Button>
            </div>
          }
        >
          {rejected.error}
        </Notice>
      ) : (
        <div className="grid justify-start gap-3" style={layout}>
          {original && <Original input={original} refs={refs} />}
          {group.prints.map((p) => (
            <PrintFrame key={p.key} print={p} group={group} label={original ? 'Edit' : undefined} />
          ))}
        </div>
      )}
    </section>
  );
}

/** The image an edit started from, beside the result at the same height, with its own film edge. */
function Original({ input, refs }: { input: InputImage; refs: InputImage[] }) {
  // A Library image shows at full size; a dropped or pasted file is its own object URL.
  const src = input.imageId ? api.imageUrl(input.imageId) : input.src;
  return (
    <figure className="m-0 flex min-w-0 flex-col gap-2">
      <div className="relative overflow-hidden rounded-frame shadow-[0_0_0_1px_var(--color-line)]" style={{ aspectRatio: `${input.width} / ${input.height}` }}>
        <img src={src} alt={`Original: ${input.name}`} draggable={false} className="absolute inset-0 size-full object-cover" />
        <span className="edge absolute top-2.5 left-2.5 rounded-[4px] bg-bg/75 px-1.5 py-0.5 text-text">Original</span>
      </div>
      <figcaption className="edge selectable truncate text-muted">{['Original', input.name, `${input.width} × ${input.height}`].join(' ▸ ')}</figcaption>
      {refs.length > 0 && (
        <div className="flex gap-1.5" aria-label="References">
          {refs.map((r, i) => (
            <span key={r.key} className="relative" title={`Reference ${i + 2}: ${r.name}`}>
              <img src={r.src} alt={`Reference ${i + 2}`} draggable={false} className="size-10 rounded-[3px] object-cover shadow-[0_0_0_1px_var(--color-line)]" />
              <span className="edge absolute bottom-0.5 left-0.5 rounded-[2px] bg-bg/85 px-1 text-[9.5px] leading-[13px] text-text">{i + 2}</span>
            </span>
          ))}
        </div>
      )}
    </figure>
  );
}

