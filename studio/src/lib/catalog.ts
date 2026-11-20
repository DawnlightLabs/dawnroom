// The Models catalogue: one card per model family, filtered and sorted like the site's /search, but
// against this machine (the engine's own fit). Pure functions, so the filters are testable.

import type { Model, ModelList, Task } from '../api/types';
import { canDo, fitFor, runsHere } from './models';

export type Family = ModelList['families'][number] & { custom?: boolean };

export interface Card {
  id: string;
  /** Every size tag, smallest first. */
  variants: Model[];
  /** The tag the card shows: the installed or busy one, else the engine's default for this machine. */
  shown: Model;
}

export type Show = 'all' | 'installed' | 'available';
export type SizeBucket = '' | 'small' | 'medium' | 'large' | 'huge';
export type LicenceFilter = '' | 'commercial' | 'restricted';
export type Sort = 'best' | 'smallest' | 'largest' | 'name';

export interface Filters {
  q: string;
  show: Show;
  runsHere: boolean;
  task: '' | Task;
  licence: LicenceFilter;
  size: SizeBucket;
  tags: string[];
  sort: Sort;
}

export const NO_FILTERS: Filters = { q: '', show: 'all', runsHere: false, task: '', licence: '', size: '', tags: [], sort: 'best' };

/** Download sizes, by the smallest tag: what it takes to try the model at all. */
export const SIZES: { id: Exclude<SizeBucket, ''>; label: string; max: number }[] = [
  { id: 'small', label: 'Under 5 GB', max: 5e9 },
  { id: 'medium', label: '5–10 GB', max: 10e9 },
  { id: 'large', label: '10–20 GB', max: 20e9 },
  { id: 'huge', label: 'Over 20 GB', max: Infinity },
];

export const TAG_LABELS: Record<string, string> = { fast: 'Fast', quality: 'Quality', text: 'Text in images', photo: 'Photo', photoreal: 'Photoreal', sdxl: 'SDXL', small: 'Small', big: 'Big', classic: 'Classic' };

const onDisk = (m: Model) => m.state === 'installed' || m.state === 'verifying' || m.state === 'needs_test' || m.state === 'test_failed';
const busy = (m: Model) => m.state === 'pulling' || m.state === 'partial' || !!m.paused;

export const isInstalled = (c: Card) => c.variants.some(onDisk);
export const isClean = (m: Model) => m.licence.commercial && !m.licence.use_restrictions;
/** The task a card is judged on: the one asked for, else what it mainly does. */
const taskFor = (c: Card, task: '' | Task): Task => task || (canDo(c.shown, 'generate') ? 'generate' : 'edit');
/** Runs here (fits or with offload) in at least one tag. */
export const cardRunsHere = (c: Card, task: '' | Task = '') => c.variants.some((v) => runsHere(fitFor(v, taskFor(c, task))));
export const smallest = (c: Card) => Math.min(...c.variants.map((v) => v.size_bytes));
export const largest = (c: Card) => Math.max(...c.variants.map((v) => v.size_bytes));
export const sizeBucket = (bytes: number): Exclude<SizeBucket, ''> => SIZES.find((s) => bytes < s.max)!.id;

/** Groups the engine's models into one card per family. */
export function cardsOf(models: Model[], families: Family[]): Card[] {
  return families
    .map((f) => {
      const variants = f.variants.map((id) => models.find((m) => m.id === id)).filter((m): m is Model => !!m);
      if (!variants.length) return null;
      const shown = variants.find((m) => m.state === 'pulling') ?? variants.find(onDisk) ?? variants.find(busy) ?? variants.find((m) => m.id === f.default) ?? variants[0];
      return { id: f.id, variants, shown };
    })
    .filter((c): c is Card => !!c);
}

function matchesText(c: Card, q: string): boolean {
  const n = q.trim().toLowerCase();
  if (!n) return true;
  const m = c.shown;
  return [c.id, m.name, m.tagline, m.licence.name, m.licence.spdx, m.custom?.source, ...m.tags, ...c.variants.map((v) => v.tag)].some((x) => x?.toLowerCase().includes(n));
}

/** Every filter except `except` (so each sidebar option can count what it would show). */
export function matches(c: Card, f: Filters, except?: keyof Filters): boolean {
  if (except !== 'q' && !matchesText(c, f.q)) return false;
  if (except !== 'show' && f.show === 'installed' && !isInstalled(c)) return false;
  if (except !== 'show' && f.show === 'available' && isInstalled(c)) return false;
  if (except !== 'runsHere' && f.runsHere && !cardRunsHere(c, f.task)) return false;
  if (except !== 'task' && f.task && !canDo(c.shown, f.task)) return false;
  if (except !== 'licence' && f.licence === 'commercial' && !isClean(c.shown)) return false;
  if (except !== 'licence' && f.licence === 'restricted' && isClean(c.shown)) return false;
  if (except !== 'size' && f.size && sizeBucket(smallest(c)) !== f.size) return false;
  if (except !== 'tags' && f.tags.length && !f.tags.every((t) => c.shown.tags.includes(t))) return false;
  return true;
}

/** "Best here": installed first, then what runs here (fits before offload), then name. */
export function sortCards(cards: Card[], f: Pick<Filters, 'sort' | 'task'>): Card[] {
  const name = (a: Card, b: Card) => a.shown.name.localeCompare(b.shown.name) || a.id.localeCompare(b.id);
  const fitRank = (c: Card) => {
    const tiers = c.variants.map((v) => fitFor(v, taskFor(c, f.task)).tier);
    return tiers.includes('fits') ? 0 : tiers.includes('offload') ? 1 : 2;
  };
  const by: Record<Sort, (a: Card, b: Card) => number> = {
    best: (a, b) => Number(isInstalled(b)) - Number(isInstalled(a)) || fitRank(a) - fitRank(b) || name(a, b),
    smallest: (a, b) => smallest(a) - smallest(b) || name(a, b),
    largest: (a, b) => largest(b) - largest(a) || name(a, b),
    name,
  };
  return [...cards].sort(by[f.sort]);
}

export const filterCards = (cards: Card[], f: Filters) => sortCards(cards.filter((c) => matches(c, f)), f);

/** How many cards an option would show with the other filters as they are. */
export const countWith = (cards: Card[], f: Filters, patch: Partial<Filters>) => cards.filter((c) => matches(c, { ...f, ...patch })).length;

export const activeFilters = (f: Filters) =>
  [f.q.trim() !== '', f.show !== 'all', f.runsHere, f.task !== '', f.licence !== '', f.size !== '', f.tags.length > 0].filter(Boolean).length;
