import { describe, expect, it } from 'vitest';
import type { ImageItem } from '../api/types';
import { chainOf, dayLabel, editsOf, sessionsOf } from './library';

const min = 60 * 1000;
const item = (id: string, at: number, prompt: string, parent_id: string | null = null): ImageItem =>
  ({ id, created_ms: at * min, prompt, parent_id, frame: 0, seed: 1, model: 'm', params: { prompt, negative_prompt: '', width: 1024, height: 1024, seed: 1, steps: 8, cfg: 1 }, status: 'completed', kind: 'image', started_ms: null, finished_ms: null, output_path: null, error: null, favourite: false, url: '' }) as ImageItem;

const items = [
  item('a', 0, 'A lighthouse'),
  item('b', 2, 'A lighthouse'),
  item('c', 5, 'A cup on a table'),
  item('d', 7, 'make it night', 'c'),
  item('e', 9, 'add candles', 'd'),
  // A long break: a new sitting.
  item('f', 200, 'A neon sign'),
];

describe('the Library', () => {
  it('splits sittings at long gaps, newest first', () => {
    const s = sessionsOf(items);
    expect(s.map((x) => x.count)).toEqual([1, 5]);
    expect(s[1].start).toBe(0);
    expect(s[1].end).toBe(9 * min);
  });

  it('keeps a prompt and the edits made from it in one thread, newest thread first', () => {
    const [, first] = sessionsOf(items);
    expect(first.threads.map((t) => t.items.map((i) => i.id))).toEqual([['c', 'd', 'e'], ['a', 'b']]);
    expect(first.threads[0].prompt).toBe('A cup on a table');
  });

  it('walks the "edited from" chain both ways', () => {
    expect(chainOf(items[4], items).map((i) => i.id)).toEqual(['c', 'd', 'e']);
    expect(chainOf(items[0], items).map((i) => i.id)).toEqual(['a']);
    expect(editsOf(items[2], items).map((i) => i.id)).toEqual(['d']);
    // A parent that was deleted (not in the list) just ends the chain.
    expect(chainOf(item('x', 1, 'p', 'gone'), items).map((i) => i.id)).toEqual(['x']);
  });

  it('names days plainly', () => {
    const now = new Date(2026, 10, 17, 22).getTime();
    expect(dayLabel(new Date(2026, 10, 17, 9).getTime(), now)).toBe('Today');
    expect(dayLabel(new Date(2026, 10, 16, 23).getTime(), now)).toBe('Yesterday');
  });
});
