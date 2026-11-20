// "Add a model": one continuous card with two ways in, Hugging Face (any link form people copy) or a
// file already on this computer (picked, pasted, or found in the usual folders); the engine reads
// the header and answers with one verdict. A repo with several files shows
// its sizes grouped by bits, with the one for this machine preselected. The download, then the
// model's test image developing in the card, then "Try it".

import { useEffect, useRef, useState, type CSSProperties } from 'react';
import { api, deleteModel, fail, go, pullModels, refresh, setDraft, store, useApp } from '../app/state';
import type { CheckReport, Fit, FileOption, FitDetail, LocalScan } from '../api/types';
import { DOCS_ADD_MODEL } from '../config';
import { bytes, eta, pullTotals, waitLabel } from '../lib/format';
import { inTauri, openExternal, pickModelFile } from '../lib/tauri';
import { Grain } from './Moments';
import { Button, cx, Icon, IconButton, StatusDot, useNow } from './ui';

const FIT: Record<Fit, { label: string; dot: 'ok' | 'warn' | 'off' }> = {
  fits: { label: 'Fits', dot: 'ok' },
  offload: { label: 'RAM offload', dot: 'warn' },
  needs_more_memory: { label: 'Too big', dot: 'off' },
  no_gpu: { label: 'Needs a GPU', dot: 'off' },
};
const STEP_LABEL: Record<string, string> = { source: 'Source', format: 'Format', architecture: 'Architecture', companions: 'Text encoder and VAE', size: 'Size and memory', licence: 'Licence' };
const ROLE: Record<string, string> = { llm: 'Text encoder', t5xxl: 'T5 encoder', clip_l: 'CLIP-L', clip_g: 'CLIP-G', vae: 'VAE', llm_vision: 'Vision encoder' };
export const MODEL_EXTS = /\.(gguf|safetensors|sft)$/i;

/** A file path rather than a Hugging Face source: a drive letter, a backslash or a leading slash. */
const isPath = (s: string) => /^[a-zA-Z]:[\\/]|\\|^\//.test(s.trim());

const compact = (n: number) => (n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${Math.round(n / 1e3)}k` : String(n));

/** "Runs on your machine with RAM offload" — the verdict's first half. */
function verdict(f: FitDetail | null | undefined): string {
  if (f?.tier === 'fits') return 'Fits on your GPU';
  if (f?.tier === 'offload') return 'Runs on your machine with RAM offload';
  return 'Runs on your machine';
}

export function AddModel({ dropped, droppedPath, onClose, onShow }: { dropped?: string | null; droppedPath?: string | null; onClose: () => void; onShow: (id: string) => void }) {
  const ref = useRef<HTMLDialogElement>(null);
  const [hf, setHf] = useState('');
  const [path, setPath] = useState('');
  const [report, setReport] = useState<CheckReport | null>(null);
  const [checking, setChecking] = useState<string | null>(null);
  const [pulling, setPulling] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const [found, setFound] = useState<LocalScan | null>(null);
  const [scanning, setScanning] = useState<string | null>(null);
  const [scanError, setScanError] = useState<string | null>(null);

  const scan = async (dir?: string) => {
    setScanning(dir ?? 'usual');
    setScanError(null);
    try {
      setFound(await api.findLocal(dir));
    } catch (e) {
      setScanError(e instanceof Error ? e.message : String(e));
    } finally {
      setScanning(null);
    }
  };

  useEffect(() => {
    ref.current?.showModal();
    // A file dropped on Models in the desktop app comes with its path: check that file straight away.
    if (droppedPath) {
      setPath(droppedPath);
      void check(droppedPath);
    } else void scan();
  }, []);

  const check = async (source: string, keep = false) => {
    const v = source.trim().replace(/^"|"$/g, '');
    if (!v) return;
    setChecking(v);
    if (!keep) setReport(null);
    try {
      const r = await api.checkCustom({ source: v });
      // A folder: list the models in it instead.
      if (r.code === 'not_a_file') {
        setChecking(null);
        setReport(null);
        void scan(v);
        return;
      }
      setReport(r);
    } catch (e) {
      // 400 invalid_source: say it under the field it came from.
      setReport({ ok: false, source: v, id: null, name: null, label: null, steps: [], code: 'invalid_source', message: e instanceof Error ? e.message : String(e) });
    } finally {
      setChecking(null);
    }
  };

  const local = !!report && !report.repo && report.steps[0]?.status === 'ok';
  const back = () => (setReport(null), setChecking(null));

  const start = async () => {
    if (!report?.ok || !report.id) return;
    setStarting(true);
    try {
      // A file is imported first (a hard link, no second copy); the pull fetches companions and runs the test image.
      const id = local ? (await api.importCustom(report.source)).id : report.id;
      await refresh();
      pullModels([id]);
      setPulling(id);
    } catch (e) {
      fail(e);
    } finally {
      setStarting(false);
    }
  };

  const browse = async () => {
    const p = await pickModelFile('Pick a model file');
    if (p) (setPath(p), void check(p));
  };

  const showing = report || checking;

  return (
    <dialog ref={ref} onClose={onClose} aria-labelledby="add-title" className="add-card m-auto max-h-[calc(100vh-32px)] w-[760px] max-w-[calc(100vw-32px)] overflow-y-auto rounded-card border border-line-strong bg-surface p-0 text-text backdrop:bg-black/60">
      <div className="flex flex-col gap-6 px-7 pt-6 pb-7">
        <div className="flex items-start justify-between gap-4">
          <div className="flex flex-col gap-1">
            <h2 id="add-title" className="m-0 text-[18px] font-semibold">
              Add a model
            </h2>
            {!showing && !pulling && <p className="m-0 text-[13.5px] text-muted">An image model that isn't in Dawnroom's list: from Hugging Face, or a file you already downloaded.</p>}
          </div>
          <IconButton icon="close" label="Close" size={32} onClick={onClose} className="border-transparent" />
        </div>

        {pulling ? (
          <Developing id={pulling} report={report!} onClose={onClose} />
        ) : showing ? (
          <>
            <div className="flex items-center gap-3 rounded-control border border-line bg-bg px-3 py-2">
              <span className="flex-none text-[12px] text-muted">{local || isPath(report?.source ?? checking ?? '') ? 'File' : 'Hugging Face'}</span>
              <span className="selectable min-w-0 flex-1 truncate font-mono text-[12.5px]" title={report?.source ?? checking ?? ''}>
                {report?.source ?? checking}
              </span>
              <Button size="sm" variant="ghost" icon="back" onClick={back}>
                Change
              </Button>
            </div>
            {checking && !report && <Reading />}
            {report && <Result report={report} local={local} checking={checking} onPick={(f) => void check(`hf.co/${report.repo!.id}:${f}`, true)} onStart={() => void start()} starting={starting} onShow={onShow} />}
          </>
        ) : (
          <>
            <Way n={1} title="From Hugging Face" hint="Paste the model's link from huggingface.co, a link to one of its files, or just user/model.">
              <form
                onSubmit={(e) => {
                  e.preventDefault();
                  void check(hf);
                }}
                className="flex gap-2"
              >
                <input
                  autoFocus
                  value={hf}
                  onChange={(e) => setHf(e.target.value)}
                  onPaste={(e) => {
                    const t = e.clipboardData.getData('text').trim();
                    if (t) (e.preventDefault(), setHf(t), void check(t));
                  }}
                  placeholder="huggingface.co/unsloth/Z-Image-Turbo-GGUF"
                  aria-label="Hugging Face link or user/model"
                  spellCheck={false}
                  className="selectable h-11 min-w-0 flex-1 rounded-control border border-line-strong bg-bg px-3.5 font-mono text-[13px] text-text outline-0 placeholder:text-muted/70 focus-visible:border-text/40"
                />
                <Button type="submit" disabled={!hf.trim()} className="h-11 border-line-strong px-4">
                  Check
                </Button>
              </form>
            </Way>

            <Way n={2} title="From this computer" hint="A .gguf or .safetensors file you already downloaded, for example from ComfyUI or Hugging Face. Dawnroom links it in; nothing is copied or moved.">
              {dropped && <p className="m-0 rounded-control border border-line bg-bg px-3 py-2 text-[12.5px] leading-snug">Studio can't see where a dropped file lives. {inTauri ? <>Use <strong className="font-medium">Choose a file</strong> or paste the path of</> : 'Paste the path of'} <span className="font-mono text-[12px]">{dropped}</span>.</p>}
              <form
                onSubmit={(e) => {
                  e.preventDefault();
                  void check(path);
                }}
                className="flex gap-2"
              >
                {inTauri && (
                  <Button type="button" icon="folder" onClick={() => void browse()} className="h-11 px-4">
                    Choose a file…
                  </Button>
                )}
                <input
                  value={path}
                  onChange={(e) => setPath(e.target.value)}
                  placeholder={inTauri ? 'or paste a file or folder path' : 'Paste a file or folder path, e.g. D:\\ComfyUI\\models'}
                  aria-label="Path of a model file or a folder"
                  spellCheck={false}
                  className="selectable h-11 min-w-0 flex-1 rounded-control border border-line-strong bg-bg px-3.5 font-mono text-[13px] text-text outline-0 placeholder:text-muted/70 focus-visible:border-text/40"
                />
                <Button type="submit" disabled={!path.trim()} className="h-11 border-line-strong px-4">
                  Check
                </Button>
              </form>
              <Found scan={found} scanning={scanning} error={scanError} onPick={(p) => (setPath(p), void check(p))} onShow={onShow} onRescan={() => void scan()} />
            </Way>

            <HowItWorks />
          </>
        )}
      </div>
    </dialog>
  );
}

/** One way in: a numbered heading, one line of help, then its controls. */
function Way({ n, title, hint, children }: { n: number; title: string; hint: string; children: React.ReactNode }) {
  return (
    <section className="grid grid-cols-[28px_minmax(0,1fr)] gap-x-3 gap-y-3">
      <span className="edge flex size-7 items-center justify-center rounded-full border border-line-strong text-[12px] text-text">{n}</span>
      <div className="flex flex-col gap-0.5 pt-0.5">
        <h3 className="m-0 text-[14.5px] font-medium">{title}</h3>
        <p className="m-0 text-[12.5px] leading-snug text-muted">{hint}</p>
      </div>
      <div className="col-start-2 flex flex-col gap-3">{children}</div>
    </section>
  );
}

/** Image models Dawnroom found on this computer (the usual folders, or the folder pasted). */
function Found({ scan, scanning, error, onPick, onShow, onRescan }: { scan: LocalScan | null; scanning: string | null; error: string | null; onPick: (path: string) => void; onShow: (id: string) => void; onRescan: () => void }) {
  if (scanning) {
    return (
      <div className="flex items-center gap-2.5 text-[12.5px] text-muted" aria-live="polite">
        <span className="reading-dot size-1.5 rounded-full bg-safelight" />
        {scanning === 'usual' ? 'Looking in the usual places…' : `Looking in ${scanning}…`}
      </div>
    );
  }
  if (error) return <p className="m-0 text-[12.5px] text-text">{error}</p>;
  if (!scan) return null;
  const folder = scan.roots.length === 1 && scan.roots[0].label === 'This folder' ? scan.roots[0].path : null;
  const looked = scan.roots.filter((r) => r.exists);
  const places = [...new Set(scan.roots.map((r) => r.label))].join(', ');
  const others = scan.skipped > 0 ? ` ${scan.skipped} other model ${scan.skipped === 1 ? 'file isn’t an image model' : 'files aren’t image models'} (LLMs, text encoders, VAEs).` : '';
  if (scan.found.length === 0) {
    return (
      <p className="m-0 text-[12.5px] leading-snug text-muted">
        {folder ? `No image models in ${folder}.` : looked.length ? `No image models found in ${places}.` : `None of the usual folders (${places}) are on this computer.`}
        {others}
        {folder && (
          <button type="button" onClick={onRescan} className="ml-1 text-text hover:underline">
            Look in the usual places
          </button>
        )}
      </p>
    );
  }
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-baseline justify-between text-[12px] text-muted">
        <span>{folder ? `In ${folder}` : 'Found on this computer'}</span>
        {folder && (
          <button type="button" onClick={onRescan} className="hover:text-text">
            The usual places
          </button>
        )}
      </div>
      <div className="flex max-h-[232px] flex-col overflow-y-auto rounded-control border border-line">
        {scan.found.map((f) => {
          const known = f.added ?? f.library;
          const fit = f.fit ? FIT[f.fit] : null;
          return (
            <div key={f.path} title={f.path} className="grid min-h-[52px] grid-cols-[minmax(0,1fr)_64px_118px_104px] items-center gap-3 border-b border-line px-3 py-2 last:border-b-0">
              <span className="flex min-w-0 flex-col gap-0.5">
                <span className="truncate text-[13px]">{f.file}</span>
                <span className="truncate text-[11.5px] text-muted">{[f.architecture, f.place].filter(Boolean).join(' · ')}</span>
              </span>
              <span className="edge text-right text-muted">{bytes(f.size)}</span>
              <span className="flex items-center gap-1.5 text-[12px]">
                {known ? (
                  <span className="text-muted">{f.added ? 'Already added' : 'In the library'}</span>
                ) : f.ok && fit ? (
                  <>
                    <StatusDot state={fit.dot} />
                    {fit.label}
                  </>
                ) : (
                  <span className="truncate text-muted" title={f.message ?? ''}>
                    Can't run here
                  </span>
                )}
              </span>
              <span className="flex justify-end">
                {known ? (
                  <Button size="sm" variant="ghost" onClick={() => onShow(known)}>
                    Show
                  </Button>
                ) : (
                  <Button size="sm" onClick={() => onPick(f.path)}>
                    {f.ok ? 'Add' : 'Why'}
                  </Button>
                )}
              </span>
            </div>
          );
        })}
      </div>
      {others && <span className="text-[11.5px] text-muted">{others.trim()}</span>}
    </div>
  );
}

/** A short "how it works", with the full guide on the docs site. */
function HowItWorks() {
  const [open, setOpen] = useState(false);
  return (
    <section className="rounded-control border border-line">
      <button type="button" aria-expanded={open} onClick={() => setOpen((o) => !o)} className="flex w-full items-center justify-between gap-3 px-4 py-3 text-left text-[13px] hover:bg-raised/50">
        <span className="flex items-center gap-2">
          <Icon name="info" size={15} />
          How adding a model works
        </span>
        <span className={cx('text-muted transition-transform', open && 'rotate-180')}>
          <Icon name="chevron" size={14} />
        </span>
      </button>
      {open && (
        <div className="flex flex-col gap-3 border-t border-line px-4 pt-3 pb-4">
          <ol className="m-0 flex list-none flex-col gap-2.5 p-0 text-[12.5px] leading-snug">
            {GUIDE.map(([t, d], i) => (
              <li key={t} className="grid grid-cols-[18px_minmax(0,1fr)] gap-2">
                <span className="edge text-muted">{i + 1}</span>
                <span>
                  <span className="font-medium">{t}.</span> <span className="text-muted">{d}</span>
                </span>
              </li>
            ))}
          </ol>
          <Button size="sm" icon="open" onClick={() => openExternal(DOCS_ADD_MODEL)} className="w-fit">
            Open the full guide
          </Button>
        </div>
      )}
    </section>
  );
}

const GUIDE: [string, string][] = [
  ['Point at the model', 'A Hugging Face link or user/model, or a .gguf / .safetensors file on this computer.'],
  ['Dawnroom reads its header', 'Only the first few hundred KB, nothing else downloads. It works out what the model is and whether it runs on your GPU.'],
  ['Pick a size', 'When a repo has several, the one that suits this machine is picked for you. Smaller sizes need less memory and look a little rougher.'],
  ['Download and test', 'It fetches the model plus any text encoder or VAE you don\u2019t have, then makes one small test image. The model is ready once that image comes out.'],
];

function Reading() {
  return (
    <div className="flex items-center gap-2.5 text-[12.5px] text-muted" aria-live="polite">
      <span className="reading-dot size-1.5 rounded-full bg-safelight" />
      Reading the file's header…
    </div>
  );
}

function Result({ report: r, local, checking, onPick, onStart, starting, onShow }: { report: CheckReport; local: boolean; checking: string | null; onPick: (file: string) => void; onStart: () => void; starting: boolean; onShow: (id: string) => void }) {
  const [details, setDetails] = useState(false);
  const failedEarly = !r.repo && !local; // the source itself didn't resolve
  const files = r.files ?? [];
  const missing = (r.companions ?? []).filter((c) => !c.on_disk).reduce((n, c) => n + c.size, 0);
  const download = local ? missing : (r.download_bytes ?? 0);
  const recommended = files.find((f) => f.recommended);
  const failedStep = r.steps.find((s) => s.status === 'failed')?.step;
  const inLibrary = r.code === 'in_library' || r.code === 'already_added' ? (r.steps[0]?.detail?.model as string | undefined) : undefined;

  return (
    <div className="enter flex flex-col gap-5">
      {!failedEarly && <Meet report={r} local={local} />}

      {/* Sizes only when they mean something here: sized for this machine, or one to pick instead. */}
      {files.length > 1 && (files.some((f) => f.fit) || r.code === 'unsupported_quant') && <Sizes files={files} checking={checking} onPick={onPick} />}

      {r.steps.length > 0 && !checking && (
        <div className="flex flex-col gap-2">
          <button type="button" aria-expanded={details} onClick={() => setDetails((d) => !d)} className="flex w-fit items-center gap-1 text-[12px] text-muted hover:text-text">
            Details
            <span className={cx('transition-transform', details && 'rotate-180')}>
              <Icon name="chevron" size={12} />
            </span>
          </button>
          {details && (
            <ol className="m-0 flex list-none flex-col gap-2 border-l border-line pl-3">
              {r.steps.map((s) => (
                <li key={s.step} className={cx('flex flex-col gap-0.5 text-[12px]', s.status === 'skipped' && 'opacity-50')}>
                  <span className={cx('font-medium', s.step === failedStep ? 'text-danger' : 'text-text')}>
                    {STEP_LABEL[s.step] ?? s.step}
                    {s.status === 'skipped' && <span className="font-normal text-muted"> · not checked</span>}
                  </span>
                  {s.status !== 'skipped' && <span className="selectable leading-snug text-muted">{s.message}</span>}
                </li>
              ))}
            </ol>
          )}
        </div>
      )}

      {/* The verdict and its action stay on the card's bottom edge, however long the size list is. */}
      <div className="sticky bottom-0 -mx-5 -mb-5 flex flex-col gap-3.5 border-t border-line bg-surface px-5 pt-4 pb-5">
      <section className="flex flex-col gap-2.5" aria-live="polite">
        {checking ? (
          <Reading />
        ) : r.ok ? (
          <>
            <p className="m-0 text-[16px] leading-snug font-medium">
              {verdict(r.fit)}
              <span className="text-muted"> · {download > 0 ? `${bytes(download)} to download` : 'nothing to download'}</span>
            </p>
            <div className="flex flex-wrap items-center gap-1.5 text-[12px]">
              {(r.companions ?? []).length === 0 ? (
                <Chip>Carries its own text encoder and VAE</Chip>
              ) : (
                (r.companions ?? []).map((c) => (
                  <Chip key={c.file} title={c.file}>
                    {ROLE[c.role] ?? c.role} <span className="text-muted">{c.on_disk ? 'on disk' : bytes(c.size)}</span>
                  </Chip>
                ))
              )}
              {r.licence && (
                <Chip title={r.licence.note}>
                  {r.licence.url ? (
                    <button type="button" onClick={() => openExternal(r.licence!.url)} className="flex items-center gap-1 hover:underline">
                      {r.licence.name ?? r.licence.spdx}
                      <Icon name="open" size={11} />
                    </button>
                  ) : (
                    (r.licence.name ?? r.licence.spdx)
                  )}
                </Chip>
              )}
              <Chip muted>{r.label ?? 'Custom, not reviewed by Dawnroom'}</Chip>
            </div>
          </>
        ) : (
          <p className="m-0 text-[15px] leading-snug">{r.message}</p>
        )}
      </section>

      {!checking && (r.ok || (r.code === 'unsupported_quant' && recommended) || inLibrary) && (
        <div className="flex justify-end gap-2">
          {r.ok && (
            <Button variant="solid" icon={local && download === 0 ? 'plus' : 'download'} disabled={starting} onClick={onStart}>
              {local ? (download > 0 ? `Add · ${bytes(download)} to download` : 'Add and test') : `Download ${bytes(download)}`}
            </Button>
          )}
          {!r.ok && r.code === 'unsupported_quant' && recommended && <Button onClick={() => onPick(recommended.file)}>Check {recommended.quant} instead</Button>}
          {inLibrary && <Button onClick={() => onShow(inLibrary)}>Show it in Models</Button>}
        </div>
      )}
      </div>
    </div>
  );
}

function Chip({ children, title, muted }: { children: React.ReactNode; title?: string; muted?: boolean }) {
  return (
    <span title={title} className={cx('flex h-6 items-center gap-1 rounded-full border border-line px-2.5', muted ? 'text-muted' : 'text-text')}>
      {children}
    </span>
  );
}

/** Who made it and what it's built on (Hugging Face), or the file itself (local). */
function Meet({ report: r, local }: { report: CheckReport; local: boolean }) {
  const size = r.steps[0]?.detail?.size as number | undefined;
  if (local) {
    return (
      <section className="flex flex-col gap-1">
        <span className="truncate text-[15px] font-medium" title={r.source}>
          {r.file ?? r.source}
        </span>
        <span className="edge truncate text-muted">{[size != null && bytes(size), r.architecture, 'on this computer'].filter(Boolean).join(' · ')}</span>
      </section>
    );
  }
  const repo = r.repo!;
  const [author, name] = repo.id.split('/');
  const facts = [repo.base_model && `from ${repo.base_model}`, repo.licence, repo.downloads != null && `${compact(repo.downloads)} downloads`].filter(Boolean);
  return (
    <section className="flex flex-col gap-1">
      <div className="flex items-baseline gap-2">
        <span className="min-w-0 truncate text-[15px] font-medium" title={repo.id}>
          {name}
        </span>
        <span className="flex-none text-[12.5px] text-muted">by {repo.author ?? author}</span>
        <button type="button" onClick={() => openExternal(repo.url)} aria-label="Open on Hugging Face" className="ml-auto flex flex-none items-center gap-1 text-[12px] text-muted hover:text-text">
          Hugging Face <Icon name="open" size={11} />
        </button>
      </div>
      <span className="edge truncate text-muted">{facts.join(' · ')}</span>
    </section>
  );
}

/** Sizes grouped by bits, like Hugging Face's own panel; formats sd.cpp can't load fold away. */
function Sizes({ files, checking, onPick }: { files: FileOption[]; checking: string | null; onPick: (file: string) => void }) {
  const [showNo, setShowNo] = useState(false);
  const ok = files.filter((f) => f.loadable);
  const no = files.filter((f) => !f.loadable);
  const groups = [...new Set(ok.map((f) => f.bits))].sort((a, b) => (a ?? 99) - (b ?? 99));
  const picking = checking?.split(':').slice(1).join(':') ?? null;
  return (
    <section className="flex flex-col gap-1" aria-label="Choose a size">
      <div className="flex items-baseline justify-between pb-1 text-[12px] text-muted">
        <span>Choose a size</span>
        <span>Fit on this machine (estimate)</span>
      </div>
      <div role="radiogroup" className="flex flex-col">
        {groups.map((b) => (
          <div key={b ?? 'other'} className="grid grid-cols-[52px_minmax(0,1fr)] border-t border-line first:border-t-0">
            <span className="edge pt-2.5 text-muted">{b != null ? `${b}-bit` : 'other'}</span>
            <div className="flex flex-col py-1">
              {ok
                .filter((f) => f.bits === b)
                .map((f) => {
                  const on = picking ? f.file === picking : f.selected;
                  const fit = f.fit ? FIT[f.fit] : null;
                  return (
                    <button key={f.file} type="button" role="radio" aria-checked={on} title={f.file} onClick={() => !on && onPick(f.file)} className={cx('grid h-9 grid-cols-[14px_minmax(0,1fr)_76px_104px] items-center gap-3 rounded-control px-2 text-left text-[12.5px] transition-colors', on ? 'bg-raised' : 'hover:bg-raised/60')}>
                      <span className={cx('flex size-3 items-center justify-center rounded-full border-[1.5px]', on ? 'border-text' : 'border-line-strong')}>{on && <span className="size-1.5 rounded-full bg-text" />}</span>
                      <span className="flex min-w-0 items-baseline gap-2">
                        <span className="truncate font-mono text-[12.5px]">{f.quant}</span>
                        <span className="flex-none font-mono text-[10.5px] text-muted">{f.format}</span>
                        {f.recommended && <span className="truncate rec text-[11.5px]">Recommended for this machine</span>}
                      </span>
                      <span className="edge text-right text-muted">{bytes(f.size)}</span>
                      <span className="flex items-center gap-1.5 text-[12px]">
                        {fit && <StatusDot state={fit.dot} />}
                        <span className={fit?.dot === 'off' ? 'text-muted' : 'text-text'}>{fit?.label ?? '—'}</span>
                      </span>
                    </button>
                  );
                })}
            </div>
          </div>
        ))}
      </div>
      {no.length > 0 && (
        <div className="border-t border-line pt-2">
          <button type="button" aria-expanded={showNo} onClick={() => setShowNo((s) => !s)} className="flex items-center gap-1 text-[12px] text-muted hover:text-text">
            {no.length} {no.length === 1 ? 'file in a format' : 'files in formats'} Dawnroom can't run yet
            <span className={cx('transition-transform', showNo && 'rotate-180')}>
              <Icon name="chevron" size={12} />
            </span>
          </button>
          {showNo && (
            <ul className="m-0 mt-1.5 flex list-none flex-col gap-1 p-0 pl-[52px] opacity-60">
              {no.map((f) => (
                <li key={f.file} title={f.file} className="grid grid-cols-[minmax(0,130px)_60px_minmax(0,1fr)] gap-3 text-[12px]">
                  <span className="truncate font-mono">{f.quant}</span>
                  <span className="edge text-right text-muted">{bytes(f.size)}</span>
                  <span className="text-muted leading-snug">{f.reason}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </section>
  );
}

/** The payoff: the download, then the test image developing in the card, then "Try it". */
function Developing({ id, report, onClose }: { id: string; report: CheckReport; onClose: () => void }) {
  const model = useApp((s) => s.models.find((m) => m.id === id));
  const live = useApp((s) => s.pulls[id]);
  const now = useNow();
  const p = live ?? model?.pull ?? null;
  const test = model?.custom?.test ?? null;
  const state = model?.state;
  const testing = p?.stage === 'testing';
  const done = state === 'installed' && test?.ok;
  const failed = state === 'test_failed' || (test && !test.ok && state !== 'pulling');
  const pullError = model?.pull_error;
  const totals = model ? pullTotals(model, p) : { done: 0, total: report.download_bytes ?? 0, fraction: 0 };
  const develop = testing ? (p.total ? p.bytes / p.total : 0.05) : done ? 1 : 0;
  const speed = p?.stage === 'downloading' && p.speed_bps > 0 ? p.speed_bps : 0;
  const left = speed ? (totals.total - totals.done) / speed : null;

  const status = done
    ? 'Ready'
    : failed
      ? 'The test image didn’t pass'
      : pullError
        ? pullError.message
        : testing
          ? `Developing its test image${p.total ? ` · ${p.bytes}/${p.total}` : ''}…`
          : p?.stage === 'waiting' && p.wait
            ? waitLabel(p.wait, now)
            : p?.stage === 'verifying'
              ? 'Checking the download…'
              : p
                ? `Downloading · ${bytes(totals.done)} of ${bytes(totals.total)}${speed ? ` · ${bytes(speed)}/s${left != null ? ` · ${eta(left)}` : ''}` : ''}`
                : 'Starting…';

  const tryIt = () => {
    const d = store.get().draft;
    setDraft({ task: 'generate', models: { ...d.models, generate: id } });
    go('create');
    onClose();
  };

  return (
    <div className="enter flex flex-col gap-4" aria-live="polite">
      <div className="flex flex-col gap-1">
        <span className="truncate text-[15px] font-medium">{model?.name ?? report.name}</span>
        <span className="edge truncate text-muted">{id}</span>
      </div>
      <div className="grid grid-cols-[220px_minmax(0,1fr)] items-end gap-5">
        <figure className="m-0 flex flex-col gap-1.5">
          <div className="relative aspect-square overflow-hidden rounded-[6px] bg-bg shadow-[var(--shadow-print)]">
            {done && test?.job ? (
              <TestPrint job={test.job} />
            ) : failed && test?.job ? (
              <img src={api.imageUrl(test.job, 512)} alt="The failed test image" className="absolute inset-0 size-full object-cover opacity-80" />
            ) : (
              <div className={cx('developing absolute inset-0', !testing && 'unexposed')} style={{ '--p': develop } as CSSProperties}>
                <div className="tint" />
                <Grain />
                {testing && p.total > 0 && <div className="edge absolute top-2 left-2 rounded-[4px] bg-bg/70 px-1.5 py-0.5 text-text">{p.bytes}/{p.total}</div>}
              </div>
            )}
          </div>
          <figcaption className="edge text-muted">seed 42{test?.steps ? ` ▸ ${test.steps} steps` : ''}{test?.seconds ? ` ▸ ${test.seconds.toFixed(1)} s` : ''}</figcaption>
        </figure>
        <div className="flex min-w-0 flex-col gap-3 pb-6">
          <p className={cx('m-0 leading-snug', done ? 'text-[18px] font-medium' : 'text-[13.5px]')}>{status}</p>
          {(done || failed) && test && <p className="m-0 text-[12.5px] leading-snug text-muted">{test.message}</p>}
          {!done && !failed && !pullError && (
            <div className="h-[3px] overflow-hidden rounded-full bg-text/10">
              <div className="h-full bg-safelight transition-[width] duration-200" style={{ width: `${(testing ? develop : totals.fraction) * 100}%` }} />
            </div>
          )}
          <div className="flex flex-wrap gap-2">
            {done && (
              <Button variant="solid" icon="next" onClick={tryIt}>
                Try it
              </Button>
            )}
            {(failed || pullError) && <Button onClick={() => pullModels([id])}>Try again</Button>}
            {failed && (
              <Button icon="trash" onClick={() => (void deleteModel(id), onClose())}>
                Remove it
              </Button>
            )}
            <Button onClick={onClose} className="border-transparent">
              {done ? 'Done' : 'Close'}
            </Button>
          </div>
          {!done && !failed && <span className="text-[12px] text-muted">It carries on if you close this.</span>}
        </div>
      </div>
    </div>
  );
}

/** The real test image, settling in with the lights-on flash once it has loaded. */
function TestPrint({ job }: { job: string }) {
  const [loaded, setLoaded] = useState(false);
  return (
    <>
      <img src={api.imageUrl(job)} alt="The model's test image: a red apple on a wooden table" onLoad={() => setLoaded(true)} className={cx('absolute inset-0 size-full object-cover', loaded ? 'settle' : 'opacity-0')} />
      {loaded && <div className="lights-on" />}
    </>
  );
}
