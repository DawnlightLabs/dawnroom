import { describe, expect, it } from 'vitest';
import type { Job, PullProgress } from '../api/types';
import { bytes, duration, eta, filmEdge, pullTotals, recipeText, shortGpu, stageProgress, pausedLabel, vram, waitLabel } from './format';

const job: Job = {
  id: 'j1',
  kind: 'image',
  status: 'completed',
  prompt: 'A sign that reads "OPEN LATE"',
  params: { prompt: 'x', negative_prompt: '', width: 1024, height: 1024, seed: 482910, steps: 8, cfg: 1 },
  seed: 482910,
  model: 'z-image-turbo',
  created_ms: 0,
  started_ms: 1000,
  finished_ms: 25800,
  output_path: 'x.png',
  error: null,
  frame: 249,
  favourite: false,
};

describe('format', () => {
  it('writes sizes, times and hardware the way the UI shows them', () => {
    expect(bytes(6696835812)).toBe('6.7 GB');
    expect(bytes(21103664506)).toBe('21 GB');
    expect(bytes(39412213)).toBe('39 MB');
    expect(duration(24812)).toBe('24.8 s');
    expect(duration(125000)).toBe('2 min 5 s');
    expect(eta(32)).toBe('about 30 s left');
    expect(eta(170)).toBe('about 3 min left');
    expect(shortGpu('NVIDIA GeForce RTX 5060 Laptop GPU')).toBe('RTX 5060 Laptop');
    expect(vram(8151)).toBe('8 GB');
    expect(vram(11264)).toBe('11 GB');
  });

  it('writes the film-edge caption and a recipe that reproduces the image', () => {
    expect(filmEdge(job)).toBe('0249 ▸ seed 482910 ▸ 8 steps ▸ 24.8 s');
    expect(filmEdge({ ...job, finished_ms: null })).toBe('0249 ▸ seed 482910 ▸ 8 steps');
    expect(recipeText(job).split('\n')).toEqual([
      'A sign that reads "OPEN LATE"',
      'z-image-turbo · seed 482910 · 8 steps · guidance 1 · 1024×1024',
      'dawnroom run "A sign that reads \\"OPEN LATE\\"" --model z-image-turbo --seed 482910',
    ]);
  });

  it('adds up pull progress across files', () => {
    const model = { size_bytes: 600, files: [100, 200, 300].map((size, i) => ({ path: `f${i}`, role: '', size, status: { state: 'missing' } })) };
    const at = (file: string, stage: PullProgress['stage'], b: number, total: number): PullProgress => ({ type: 'progress', file, file_index: 0, file_count: 3, stage, bytes: b, total, speed_bps: 1 });
    expect(pullTotals(model, null).fraction).toBe(0);
    expect(pullTotals(model, at('f1', 'downloading', 50, 200))).toEqual({ done: 150, total: 600, fraction: 0.25 });
    expect(pullTotals(model, at('f2', 'verifying', 10, 300)).done).toBe(600);
    // Runtime steps come first and don't count as the model's bytes.
    expect(pullTotals(model, at('runtime/sd-vulkan.zip', 'downloading', 20, 30)).done).toBe(0);
  });

  it('maps stages onto the developing progress', () => {
    expect(stageProgress({ stage: 'conditioning', step: null, total: null })).toBe(0);
    expect(stageProgress({ stage: 'sampling', step: 2, total: 8 })).toBe(0.25);
    expect(stageProgress({ stage: 'vae_decode', step: null, total: null })).toBe(1);
  });
});

describe('waiting and paused downloads', () => {
  it('counts down to the next attempt', () => {
    const w = { kind: 'connection' as const, reason: 'No connection', retry_at_ms: 20_000, attempt: 3 };
    expect(waitLabel(w, 8_100)).toBe('Network error, trying again in 12 s');
    expect(waitLabel(w, 21_000)).toBe('Network error, trying again now');
    expect(waitLabel({ ...w, kind: 'busy', reason: 'Hugging Face is busy' }, 17_000)).toBe('Hugging Face is busy, trying again in 3 s');
  });
  it('says where a paused download stopped', () => {
    expect(pausedLabel({ bytes: 4_100_000_000, total: 13_000_000_000, updated_ms: null })).toBe('Your download stopped at 4.1 of 13.0 GB.');
    expect(pausedLabel({ bytes: 12_000_000, total: 30_000_000, updated_ms: 1 })).toBe('Your download stopped at 12 MB of 30 MB.');
  });
  it('counts a waiting file by its kept bytes', () => {
    const model = { size_bytes: 300, files: [{ path: 'a', role: 'vae', size: 100, status: { state: 'verified' } }, { path: 'b', role: 'diffusion_model', size: 200, status: { state: 'partial' } }] } as never;
    expect(pullTotals(model, { type: 'progress', file: 'b', file_index: 1, file_count: 2, stage: 'waiting', bytes: 50, total: 200, speed_bps: 0 }).done).toBe(150);
  });
});
