// Edit inputs, attached like a chat's: the image to edit (drop, paste, a file, the Library, "Edit this")
// and up to nine references. Inputs go to the engine once (POST /api/inputs) and are sent by sha256.

import { useEffect, useRef, useState } from 'react';
import type { ImageItem } from '../api/types';
import { api, attachFile, attachJob, removeInput, useApp, type InputImage } from '../app/state';
import { frameNo } from '../lib/format';
import { cx, Icon } from './ui';

const MAX_REFS = 9;

/** First image file in a drop or paste. */
export function imageFrom(list: DataTransferItemList | FileList | null | undefined): File | null {
  if (!list) return null;
  for (const it of Array.from(list as ArrayLike<DataTransferItem | File>)) {
    const f = it instanceof File ? it : it.kind === 'file' ? it.getAsFile() : null;
    if (f && f.type.startsWith('image/')) return f;
  }
  return null;
}

/** Choose files, or pick from the Library: the composer's attach button. Opens upward. */
export function AttachButton() {
  const [menu, setMenu] = useState(false);
  const [picking, setPicking] = useState(false);
  const full = useApp((s) => !!s.draft.image && s.draft.refs.length >= MAX_REFS);
  const file = useRef<HTMLInputElement>(null);
  return (
    <div className="relative">
      <button
        type="button"
        aria-label="Attach an image to edit"
        title="Attach an image to edit"
        aria-expanded={menu}
        disabled={full}
        onClick={() => setMenu((m) => !m)}
        className={cx('flex size-8 items-center justify-center rounded-control border transition-colors disabled:opacity-40', menu ? 'border-line-strong bg-raised text-text' : 'border-line text-muted hover:bg-raised hover:text-text')}
      >
        <Icon name="plus" size={15} />
      </button>
      <input
        ref={file}
        type="file"
        accept="image/png,image/jpeg,image/webp"
        multiple
        hidden
        onChange={(e) => {
          Array.from(e.target.files ?? []).forEach(attachFile);
          e.target.value = '';
        }}
      />
      {menu && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setMenu(false)} aria-hidden="true" />
          <div role="menu" className="enter absolute bottom-[calc(100%+8px)] left-0 z-50 flex w-[220px] flex-col rounded-card border border-line-strong bg-surface p-1 shadow-[0_18px_48px_rgb(0_0_0/0.3)]">
            <MenuItem icon="folder" onClick={() => (setMenu(false), file.current?.click())}>
              Choose files…
            </MenuItem>
            <MenuItem icon="library" onClick={() => (setMenu(false), setPicking(true))}>
              From the Library
            </MenuItem>
            <p className="m-0 px-2.5 pt-1 pb-1.5 text-[11.5px] leading-snug text-muted">Or drop or paste an image anywhere here.</p>
          </div>
        </>
      )}
      {picking && <LibraryPicker onPick={attachJob} onClose={() => setPicking(false)} />}
    </div>
  );
}

function MenuItem({ icon, onClick, children }: { icon: 'folder' | 'library'; onClick: () => void; children: React.ReactNode }) {
  return (
    <button type="button" role="menuitem" onClick={onClick} className="flex h-8 items-center gap-2 rounded-control px-2.5 text-left text-[13px] text-text hover:bg-raised">
      <span className="text-muted">
        <Icon name={icon} size={14} />
      </span>
      {children}
    </button>
  );
}

/** The attachments inside the composer, like a chat's: the image to edit (1), then references (2, 3…). */
export function Attachments() {
  const task = useApp((s) => s.draft.task);
  const image = useApp((s) => s.draft.image);
  const refs = useApp((s) => s.draft.refs);
  // Nothing attached yet: the placeholder and the + say how; no empty box taking the prints' room.
  if (task !== 'edit' || (!image && !refs.length)) return null;
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex flex-wrap items-end gap-2">
        {image && <Attached input={image} label="Editing" n={1} />}
        {refs.map((r, i) => (
          <Attached key={r.key} input={r} n={i + 2} />
        ))}
      </div>
      {refs.length > 0 && <p className="m-0 text-[11.5px] leading-snug text-muted">In the instruction, the image you edit is image 1 and the references are 2, 3…</p>}
    </div>
  );
}

function Attached({ input, n, label }: { input: InputImage; n: number; label?: string }) {
  return (
    <div className="group relative flex w-14 flex-col gap-1" title={`${input.name} · ${input.width} × ${input.height}`}>
      <Thumb input={input} size={56} />
      <span className="edge absolute top-1 left-1 rounded-[2px] bg-bg/85 px-1 text-[10px] leading-[14px] text-text">{n}</span>
      <span className="edge text-[10px] leading-none text-muted">{label ?? 'Reference'}</span>
      <button type="button" aria-label={`Remove ${label ? 'the image to edit' : `reference ${n}`}`} onClick={() => removeInput(input.key)} className="absolute -top-1.5 -right-1.5 hidden size-5 items-center justify-center rounded-full border border-line-strong bg-surface text-muted group-focus-within:flex group-hover:flex hover:text-text">
        <Icon name="close" size={11} />
      </button>
    </div>
  );
}

function Thumb({ input, size }: { input: InputImage; size: number }) {
  return (
    <div className="relative flex-none overflow-hidden rounded-[3px] bg-surface shadow-[0_0_0_1px_var(--color-line)]" style={{ width: size, height: size }}>
      <img src={input.src} alt={input.name} draggable={false} className={cx('size-full object-contain', input.status === 'loading' && 'opacity-50')} />
      {input.status === 'loading' && <span className="breathe absolute right-1 bottom-1 size-1.5 rounded-full bg-safelight-to" />}
    </div>
  );
}

/** A small contact sheet of recent Library prints to pick from. */
function LibraryPicker({ onPick, onClose }: { onPick: (item: ImageItem) => void; onClose: () => void }) {
  const [items, setItems] = useState<ImageItem[] | null>(null);
  const [q, setQ] = useState('');
  const rev = useApp((s) => s.libraryRev);
  useEffect(() => {
    const ctrl = new AbortController();
    const t = setTimeout(() => api.images({ limit: 48, q }, ctrl.signal).then((p) => setItems(p.items), () => {}), q ? 200 : 0);
    return () => {
      clearTimeout(t);
      ctrl.abort();
    };
  }, [q, rev]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    addEventListener('keydown', onKey);
    return () => removeEventListener('keydown', onKey);
  }, [onClose]);
  return (
    <>
      <div className="fixed inset-0 z-40" onClick={onClose} aria-hidden="true" />
      <div role="dialog" aria-label="Pick from the Library" className="enter absolute bottom-[calc(100%+8px)] left-0 z-50 flex max-h-[420px] w-[440px] flex-col gap-2 rounded-card border border-line-strong bg-surface p-2 shadow-[0_18px_48px_rgb(0_0_0/0.35)]">
        <div className="flex items-center gap-2 rounded-control border border-line bg-bg px-2.5">
          <Icon name="search" size={14} />
          <input autoFocus value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search your prints" aria-label="Search your prints" className="selectable h-8 flex-1 border-0 bg-transparent text-[13px] text-text outline-0 placeholder:text-muted" />
        </div>
        <div className="grid min-h-0 grid-cols-4 gap-1.5 overflow-y-auto">
          {items?.map((it) => (
            <button
              key={it.id}
              type="button"
              title={it.prompt}
              onClick={() => {
                onPick(it);
                onClose();
              }}
              className="group flex flex-col gap-0.5 rounded-[3px] text-left"
            >
              <img src={api.imageUrl(it.id, 256)} alt={it.prompt} draggable={false} loading="lazy" className="aspect-square w-full rounded-[3px] bg-surface object-cover shadow-[0_0_0_1px_var(--color-line)] group-hover:shadow-[0_0_0_2px_var(--color-ring)]" />
              <span className="edge text-[10px] text-muted">{frameNo(it.frame)}</span>
            </button>
          ))}
          {items?.length === 0 && <p className="col-span-4 m-0 p-3 text-[12.5px] text-muted">{q ? 'Nothing matches.' : 'Nothing in the Library yet.'}</p>}
        </div>
      </div>
    </>
  );
}
