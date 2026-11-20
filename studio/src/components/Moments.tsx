// The Darkroom signature moments. CSS and SVG only; the motion itself lives in styles.css
// and is switched off as a whole by <html data-motion="off">.

import { useId } from 'react';
import { isDeveloping, useApp } from '../app/state';
import { MARK } from '../lib/brand';

/** 1. Safelight on: a faint terracotta glow at the window edges while anything develops. */
export function Safelight() {
  const on = useApp(isDeveloping);
  return (
    <div className="safelight" data-on={on} aria-hidden="true">
      <div />
    </div>
  );
}

/** Film grain: SVG turbulence that steps four times a second. `strength` is its opacity where CSS doesn't set one. */
export function Grain({ strength }: { strength?: number }) {
  const id = useId();
  return (
    <div className="grain" style={strength == null ? undefined : { opacity: strength }} aria-hidden="true">
      <svg>
        <filter id={id}>
          <feTurbulence type="fractalNoise" baseFrequency="0.85" numOctaves="2" stitchTiles="stitch" />
          <feColorMatrix type="saturate" values="0" />
        </filter>
        <rect width="100%" height="100%" filter={`url(#${id})`} />
      </svg>
    </div>
  );
}

/**
 * 3. Dawn progress: the Dawnroom tile rises over a horizon line as a download completes. At 0 it is below the line; at 1 it sits clear of it, and a low
 * terracotta-to-sun glow has come up behind it.
 */
export function DawnProgress({ value, label }: { value: number; label: string }) {
  const p = Math.max(0, Math.min(1, value));
  const id = useId();
  const d = 76;
  const horizon = 104;
  return (
    <svg className="dawn block w-full" viewBox="0 0 320 124" height="124" role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(p * 100)}>
      <defs>
        <radialGradient id={`${id}g`} cx="50%" cy="100%" r="50%">
          <stop offset="0" stopColor="#C47855" stopOpacity="0.5" />
          <stop offset="0.55" stopColor="#FFD12B" stopOpacity="0.12" />
          <stop offset="1" stopColor="#FFD12B" stopOpacity="0" />
        </radialGradient>
        <clipPath id={`${id}c`}>
          <rect x="0" y="0" width="320" height={horizon} />
        </clipPath>
      </defs>
      {/* The sky warms as the tile comes up. */}
      <rect className="dawn-glow" x="40" y="0" width="240" height={horizon} fill={`url(#${id}g)`} style={{ opacity: 0.15 + 0.85 * p }} />
      <g clipPath={`url(#${id}c)`}>
        <image className="dawn-sun" href={MARK} x={160 - d / 2} y={horizon - d - 6} width={d} height={d} style={{ transform: `translateY(${(1 - p) * (d + 8)}px)` }} />
      </g>
      <path d={`M24 ${horizon + 0.5}H296`} stroke="#FCF8F5" strokeOpacity="0.7" strokeWidth="1.5" />
      {/* Its light on the water, once it clears the line. */}
      <g className="dawn-water" stroke="#FFD12B" strokeWidth="1.5" strokeLinecap="square" style={{ opacity: p * 0.6 }}>
        <path d={`M140 ${horizon + 7}H180`} />
        <path d={`M148 ${horizon + 12}H172`} />
        <path d={`M155 ${horizon + 17}H165`} />
      </g>
    </svg>
  );
}

/**
 * 4. Grease pencil: a hand-drawn circle around a favourite, the way prints are marked on a
 * contact sheet. `draw` animates the stroke (when the user has just marked it).
 */
export function GreasePencil({ draw }: { draw: boolean }) {
  const id = useId();
  return (
    <svg className="grease pointer-events-none absolute inset-0 size-full" data-draw={draw} viewBox="0 0 100 100" aria-hidden="true">
      <filter id={id} x="-5%" y="-5%" width="110%" height="110%">
        {/* A waxy, slightly broken edge. */}
        <feTurbulence type="fractalNoise" baseFrequency="0.45" numOctaves="2" seed="4" result="n" />
        <feDisplacementMap in="SourceGraphic" in2="n" scale="1.1" />
      </filter>
      <path
        d="M24 9 C50 1 88 5 94 33 C100 62 86 93 51 95.5 C17 98 2 75 5.5 45 C8 20 30 5 59 7.5 C72 8.6 82 13 89 21"
        pathLength={1}
        fill="none"
        stroke="var(--color-wax)"
        strokeOpacity="0.92"
        strokeWidth="1.7"
        strokeLinecap="round"
        filter={`url(#${id})`}
      />
    </svg>
  );
}
