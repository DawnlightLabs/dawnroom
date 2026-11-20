// Small shared pieces: icons, buttons, segmented control, progress, notices.

import { useEffect, useRef, useState, type ButtonHTMLAttributes, type ReactNode } from 'react';
import { markFor, ORB, WORDMARK_PATH, WORDMARK_VIEWBOX } from '../lib/brand';
import { openExternal } from '../lib/tauri';

// Icons: 24 grid, 1.5 stroke, square caps, mitred joins (design canvas, artboard 7).
const ICONS = {
  create: 'M4 20l1-5L16 4l4 4L9 19z',
  library: 'M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4zM13 13h7v7h-7z',
  models: 'M12 3l9 5-9 5-9-5zM3 13l9 5 9-5',
  settings: 'M4 7h9M17 7h3M4 17h3M11 17h9M13 7a2 2 0 1 0 4 0a2 2 0 1 0-4 0M7 17a2 2 0 1 0 4 0a2 2 0 1 0-4 0',
  vary: 'M4 4h16v16H4z|M9 9h.01M15 15h.01M12 12h.01',
  lock: 'M5 11h14v9H5zM8 11V8a4 4 0 0 1 8 0v3',
  unlock: 'M5 11h14v9H5zM8 11V8a4 4 0 0 1 7.5-2',
  copy: 'M8 8h12v12H8zM4 4h12v4M4 4v12h4',
  open: 'M7 17L17 7M9 7h8v8',
  check: 'M5 12.5l4.5 4.5L19 7.5',
  warn: 'M12 4L21 20H3zM12 10v4.5M12 17v.5',
  search: 'M4.5 10.5a6 6 0 1 0 12 0a6 6 0 1 0-12 0M15 15l5 5',
  back: 'M15 5l-7 7 7 7',
  next: 'M9 5l7 7-7 7',
  folder: 'M3 6h7l2 2h9v11H3z',
  trash: 'M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13',
  download: 'M12 4v11M7 10l5 5 5-5M5 20h14',
  close: 'M6 6l12 12M18 6L6 18',
  dice: 'M4 4h16v16H4z|M8.5 8.5h.01M15.5 15.5h.01M15.5 8.5h.01M8.5 15.5h.01',
  retry: 'M5 12a7 7 0 1 1 2.5 5.4M5 18v-5h5',
  stop: 'M7 7h10v10H7z',
  circle: 'M12 4.5a7.5 7.5 0 1 0 0 15 7.5 7.5 0 0 0 0-15z',
  info: 'M12 3.5a8.5 8.5 0 1 0 0 17 8.5 8.5 0 0 0 0-17zM12 11v5.5M12 7.5v1.5',
  plus: 'M12 5v14M5 12h14',
  image: 'M4 5h16v14H4zM4 16l5-5 4 4 2.5-2.5L20 17|M15.5 9h.01',
  edit: 'M4 20l1-5L16 4l4 4L9 19zM13.5 6.5l4 4',
  chevron: 'M7 10l5 5 5-5',
  paste: 'M8 5H5v15h14V5h-3M8 3h8v4H8z',
  tray: 'M4 14v6h16v-6M8 9l4-4 4 4M12 5v10',
} as const;

export type IconName = keyof typeof ICONS;

export function Icon({ name, size = 16, stroke = 1.5 }: { name: IconName; size?: number; stroke?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={stroke} strokeLinecap="square" strokeLinejoin="miter" aria-hidden="true" className="flex-none">
      {/* "outline|dots": the dots are drawn heavier so they survive at 14 px. */}
      {ICONS[name].split('|').map((d, i) => (
        <path key={i} d={d} strokeWidth={i ? 2.6 : undefined} strokeLinecap={i ? 'round' : undefined} />
      ))}
    </svg>
  );
}

/** The Dawnroom mark, the tile: gradient master from 32 px, the flat two-tone at 24 px and below. */
export function Mark({ size = 16, className = '' }: { size?: number; className?: string }) {
  return <img src={markFor(size)} width={size} height={size} alt="" draggable={false} className={`flex-none ${className}`} />;
}

/** The Dawnlight orb (the Kit's master, never redrawn): only beside "Dawnlight Labs". */
export function Orb({ size = 14, className = '' }: { size?: number; className?: string }) {
  return <img src={ORB} width={size} height={size} alt="" draggable={false} className={`flex-none ${className}`} />;
}

/** The DAWNROOM wordmark in currentColor, `height` = cap height. */
export function Wordmark({ height = 14, className = '' }: { height?: number; className?: string }) {
  return (
    <svg height={height} width={(height * 6983) / 700} viewBox={WORDMARK_VIEWBOX} role="img" aria-label="Dawnroom" className={`flex-none ${className}`}>
      <path d={WORDMARK_PATH} fill="currentColor" />
    </svg>
  );
}

/** Tile + wordmark, built like the Dawnlight lockup: the tile at 1.3 × cap height, gap 0.42 × the tile. */
export function Lockup({ cap = 14, className = '' }: { cap?: number; className?: string }) {
  const m = Math.round(cap * 1.3);
  return (
    <span className={`inline-flex items-center ${className}`} style={{ gap: m * 0.42 }}>
      <Mark size={m} />
      <Wordmark height={cap} />
    </span>
  );
}

const cx = (...parts: (string | false | null | undefined)[]) => parts.filter(Boolean).join(' ');
export { cx };

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & { variant?: 'ghost' | 'outline' | 'solid' | 'danger'; size?: 'sm' | 'md'; icon?: IconName };

/** A quiet button. The filled safelight one is DevelopButton, and there is only one of those. */
export function Button({ variant = 'outline', size = 'md', icon, className, children, ...rest }: ButtonProps) {
  return (
    <button
      type="button"
      {...rest}
      className={cx(
        'inline-flex items-center justify-center gap-1.5 rounded-control whitespace-nowrap transition-colors disabled:opacity-45',
        size === 'sm' ? 'h-[30px] px-2.5 text-[12.5px]' : 'h-9 px-3 text-[13px]',
        variant === 'outline' && 'border border-line hover:bg-raised hover:border-line-strong',
        variant === 'ghost' && 'text-muted hover:bg-raised hover:text-text',
        variant === 'solid' && 'bg-text text-bg font-semibold hover:opacity-90',
        variant === 'danger' && 'border border-line text-danger hover:bg-raised hover:border-line-strong',
        className,
      )}
    >
      {icon && <Icon name={icon} size={size === 'sm' ? 14 : 15} />}
      {children}
    </button>
  );
}

export function IconButton({ icon, label, className, size = 32, ...rest }: ButtonHTMLAttributes<HTMLButtonElement> & { icon: IconName; label: string; size?: number }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      {...rest}
      style={{ width: size, height: size }}
      className={cx('inline-flex flex-none items-center justify-center rounded-control border border-line text-muted transition-colors hover:bg-raised hover:text-text hover:border-line-strong disabled:opacity-45', className)}
    >
      <Icon name={icon} size={size <= 30 ? 14 : 16} />
    </button>
  );
}

/** Selected = raised surface. Never the accent. */
export function Segmented<T extends string>({ value, options, onChange, label, size = 'sm' }: { value: T; options: { id: T; label: ReactNode; title?: string; disabled?: boolean }[]; onChange: (v: T) => void; label: string; size?: 'sm' | 'md' }) {
  return (
    <div role="radiogroup" aria-label={label} className="flex w-fit flex-none rounded-control border border-line bg-bg p-0.5">
      {options.map((o) => (
        <button
          key={o.id}
          type="button"
          role="radio"
          aria-checked={o.id === value}
          title={o.title}
          disabled={o.disabled}
          onClick={() => onChange(o.id)}
          className={cx('rounded-[6px] px-3 text-[13px] transition-colors', size === 'sm' ? 'h-[26px]' : 'h-[30px]', o.id === value ? 'bg-raised text-text' : 'text-muted hover:text-text', 'disabled:opacity-45')}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Progress({ value, label, className }: { value: number; label: string; className?: string }) {
  const pct = Math.round(Math.max(0, Math.min(1, value)) * 100);
  return (
    <div role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct} className={cx('h-1 overflow-hidden rounded-full bg-line', className)}>
      <div className="h-full bg-safelight transition-[width] duration-500" style={{ width: `${pct}%` }} />
    </div>
  );
}

export function StatusDot({ state }: { state: 'ok' | 'busy' | 'off' | 'warn' }) {
  if (state === 'ok') return <span className="size-[7px] flex-none rounded-full bg-ok" />;
  // Works, with a caveat (e.g. RAM offload): a still sun dot, not the breathing "busy" one.
  if (state === 'warn') return <span className="size-[7px] flex-none rounded-full bg-safelight-to" />;
  if (state === 'busy') return <span className="breathe size-[7px] flex-none rounded-full bg-safelight-to" />;
  return <span className="size-[7px] flex-none rounded-full border-[1.5px] border-muted" />;
}

/** A problem in plain words, with an icon (never colour alone) and what to do next. */
export function Notice({ children, action, onDismiss, tone = 'problem' }: { children: ReactNode; action?: ReactNode; onDismiss?: () => void; tone?: 'problem' | 'info' }) {
  return (
    <div role={tone === 'problem' ? 'alert' : 'status'} className="selectable flex items-start gap-2.5 rounded-card border border-line-strong bg-surface px-3 py-2.5 text-[13px] leading-snug">
      {tone === 'problem' && (
        <span className="mt-px text-danger">
          <Icon name="warn" size={15} stroke={1.75} />
        </span>
      )}
      <div className="min-w-0 flex-1">{children}</div>
      {action}
      {onDismiss && (
        <button type="button" aria-label="Dismiss" onClick={onDismiss} className="-m-1 rounded-[6px] p-1 text-muted hover:text-text">
          <Icon name="close" size={14} />
        </button>
      )}
    </div>
  );
}

/** Copies text and says so for a moment. */
export function useCopy(): [boolean, (text: string) => void] {
  const [copied, setCopied] = useState(false);
  const timer = useRef<number>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  return [
    copied,
    (text: string) => {
      void navigator.clipboard.writeText(text).then(() => {
        setCopied(true);
        clearTimeout(timer.current);
        timer.current = window.setTimeout(() => setCopied(false), 1400);
      });
    },
  ];
}

export function CopyButton({ text, children = 'Copy', size = 'sm', className }: { text: string; children?: ReactNode; size?: 'sm' | 'md'; className?: string }) {
  const [copied, copy] = useCopy();
  return (
    <Button size={size} icon={copied ? 'check' : 'copy'} onClick={() => copy(text)} className={className}>
      {copied ? 'Copied' : children}
    </Button>
  );
}

/** Asks once more before something that can't be undone. */
export function ConfirmButton({ label, confirm, onConfirm, icon = 'trash', action = 'Delete', className }: { label: string; confirm: string; onConfirm: () => void; icon?: IconName; action?: string; className?: string }) {
  const [asking, setAsking] = useState(false);
  if (!asking)
    return (
      <Button variant="danger" icon={icon} onClick={() => setAsking(true)} className={className}>
        {label}
      </Button>
    );
  return (
    <div className={cx('flex items-center gap-2', className)}>
      <span className="flex-1 text-[12.5px] whitespace-nowrap text-muted">{confirm}</span>
      <Button size="sm" variant="ghost" autoFocus onClick={() => setAsking(false)}>
        Keep
      </Button>
      <Button
        size="sm"
        variant="danger"
        onClick={() => {
          setAsking(false);
          onConfirm();
        }}
      >
        {action}
      </Button>
    </div>
  );
}

export function PageHeader({ title, meta, children }: { title: string; meta?: ReactNode; children?: ReactNode }) {
  return (
    <header className="flex flex-none items-baseline justify-between gap-4">
      <h1 className="m-0 text-[20px] font-semibold">{title}</h1>
      {meta && <span className="edge text-[11.5px] text-muted">{meta}</span>}
      {children}
    </header>
  );
}

/** The current time, ticking once a second while mounted (countdowns). */
export function useNow(): number {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  return now;
}

/** A text link that asks once before it acts ("Discard" → "Discard? … Keep / Discard"). */
export function ConfirmLink({ label, confirm, onConfirm }: { label: string; confirm: string; onConfirm: () => void }) {
  const [asking, setAsking] = useState(false);
  const link = 'rounded-[4px] underline underline-offset-2 hover:text-text';
  if (!asking)
    return (
      <button type="button" onClick={() => setAsking(true)} className={cx(link, 'text-muted')}>
        {label}
      </button>
    );
  return (
    <span className="flex items-center gap-2.5 text-[12.5px] text-muted">
      {confirm}
      <button type="button" autoFocus onClick={() => setAsking(false)} className={link}>
        Keep
      </button>
      <button
        type="button"
        onClick={() => {
          setAsking(false);
          onConfirm();
        }}
        className={cx(link, 'text-danger')}
      >
        {label}
      </button>
    </span>
  );
}

/** "How this works": a quiet link to the docs page or anchor that explains a feature (DOCS in config). */
export function HowLink({ href, label = 'How this works', className }: { href: string; label?: string; className?: string }) {
  return (
    <button type="button" onClick={() => openExternal(href)} className={cx('inline-flex items-center gap-1 rounded-[4px] text-[12.5px] whitespace-nowrap text-muted underline underline-offset-2 hover:text-text', className)}>
      {label}
      <Icon name="open" size={11} />
    </button>
  );
}
