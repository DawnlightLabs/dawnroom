import { describe, expect, it } from 'vitest';
import type { FitDetail, Model } from '../api/types';
import { activeFilters, cardsOf, countWith, filterCards, NO_FILTERS, sizeBucket, type Family, type Filters } from './catalog';

const fits: FitDetail = { tier: 'fits', need_mib: 4403, gpu_mib: 8151, usable_ram_mib: 14336 };
const offload: FitDetail = { tier: 'offload', need_mib: 9734, gpu_mib: 8151, usable_ram_mib: 14336 };
const tooBig: FitDetail = { tier: 'needs_more_memory', need_mib: 40000, gpu_mib: 8151, usable_ram_mib: 14336 };
const clean = { spdx: 'Apache-2.0', url: 'x', commercial: true, use_restrictions: false };
const nc = { spdx: 'other', url: 'x', commercial: false, use_restrictions: true };

const model = (p: Partial<Model> & { id: string; family: string }): Model =>
  ({ name: p.family, tag: p.id.split(':')[1] ?? null, tagline: '', tags: [], size_bytes: 6e9, state: 'not_installed', tier: 'untested', fit: 'fits', fit_detail: fits, tasks: ['generate'], custom: null, licence: clean, files: [], ...p }) as Model;

const models: Model[] = [
  model({ id: 'z-image-turbo:q4', family: 'z-image-turbo', name: 'Z-Image Turbo', state: 'installed', tier: 'tested', tiers: { generate: 'tested' }, tags: ['quality', 'text'], size_bytes: 6.7e9 }),
  model({ id: 'z-image-turbo:q8', family: 'z-image-turbo', name: 'Z-Image Turbo', tier: 'untested', size_bytes: 9.4e9 }),
  model({ id: 'flux2-klein-4b:q8', family: 'flux2-klein-4b', name: 'FLUX.2 klein 4B', state: 'installed', tier: 'tested', tasks: ['generate', 'edit'], tiers: { generate: 'tested', edit: 'tested' }, tags: ['fast'], size_bytes: 7.1e9 }),
  model({ id: 'qwen-image:q4', family: 'qwen-image', name: 'Qwen-Image', tier: 'tested', fit: 'offload', fit_detail: offload, size_bytes: 18e9, tags: ['text'] }),
  model({ id: 'sd15', family: 'sd15', name: 'Stable Diffusion 1.5', tier: 'tested', size_bytes: 2.1e9, licence: nc }),
  model({ id: 'hidream:bf16', family: 'hidream', name: 'HiDream-O1', fit: 'needs_more_memory', fit_detail: tooBig, size_bytes: 35e9 }),
  model({ id: 'hf.co/a/b:c.gguf', family: 'hf.co/a/b:c.gguf', name: 'c', state: 'installed', custom: { source: 'hf.co/a/b:c.gguf' }, size_bytes: 2.7e9 }),
];
const families: Family[] = [
  { id: 'z-image-turbo', default: 'z-image-turbo:q4', variants: ['z-image-turbo:q4', 'z-image-turbo:q8'] },
  { id: 'flux2-klein-4b', default: 'flux2-klein-4b:q8', variants: ['flux2-klein-4b:q8'] },
  { id: 'qwen-image', default: 'qwen-image:q4', variants: ['qwen-image:q4'] },
  { id: 'sd15', default: 'sd15', variants: ['sd15'] },
  { id: 'hidream', default: 'hidream:bf16', variants: ['hidream:bf16'] },
  { id: 'hf.co/a/b:c.gguf', default: 'hf.co/a/b:c.gguf', variants: ['hf.co/a/b:c.gguf'], custom: true },
];
const cards = cardsOf(models, families);
const ids = (f: Partial<Filters>) => filterCards(cards, { ...NO_FILTERS, ...f }).map((c) => c.id);

describe('the Models filters', () => {
  it('makes one card per model, showing the installed tag', () => {
    expect(cards).toHaveLength(6);
    expect(cards.find((c) => c.id === 'z-image-turbo')?.shown.id).toBe('z-image-turbo:q4');
  });

  it('sorts best for this machine: installed, then fits, then offload, then what won’t run', () => {
    expect(ids({})).toEqual(['hf.co/a/b:c.gguf', 'flux2-klein-4b', 'z-image-turbo', 'sd15', 'qwen-image', 'hidream']);
    expect(ids({ sort: 'smallest' })[0]).toBe('sd15');
    expect(ids({ sort: 'largest' })[0]).toBe('hidream');
  });

  it('filters by installed, runs here, task and licence', () => {
    expect(ids({ show: 'installed' })).toEqual(['hf.co/a/b:c.gguf', 'flux2-klein-4b', 'z-image-turbo']);
    expect(ids({ show: 'available' })).toEqual(['sd15', 'qwen-image', 'hidream']);
    expect(ids({ runsHere: true })).not.toContain('hidream');
    expect(ids({ runsHere: true })).toContain('qwen-image');
    expect(ids({ task: 'edit' })).toEqual(['flux2-klein-4b']);
    expect(ids({ licence: 'restricted' })).toEqual(['sd15']);
  });

  it('filters by download size (the smallest tag), good-at tags and text', () => {
    expect(sizeBucket(2.1e9)).toBe('small');
    expect(sizeBucket(18e9)).toBe('large');
    expect(ids({ size: 'small' })).toEqual(['hf.co/a/b:c.gguf', 'sd15']);
    expect(ids({ size: 'medium' })).toEqual(['flux2-klein-4b', 'z-image-turbo']);
    expect(ids({ size: 'huge' })).toEqual(['hidream']);
    expect(ids({ tags: ['text'] })).toEqual(['z-image-turbo', 'qwen-image']);
    expect(ids({ q: 'q8' })).toEqual(['flux2-klein-4b', 'z-image-turbo']);
    expect(ids({ q: 'qwen' })).toEqual(['qwen-image']);
  });

  it('counts what each option would show with the other filters kept', () => {
    const f = { ...NO_FILTERS, show: 'installed' as const };
    expect(countWith(cards, f, { task: 'edit' })).toBe(1);
    expect(countWith(cards, f, { show: 'all' })).toBe(6);
    expect(activeFilters(f)).toBe(1);
    expect(activeFilters({ ...f, tags: ['text'], q: 'z' })).toBe(3);
  });
});
