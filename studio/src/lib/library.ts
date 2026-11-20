// The Library's shapes, derived from the image list alone (the engine has no "session"): a session is a
// sitting at the bench (prints with no gap longer than SESSION_GAP_MS between them), a thread is one line
// of work inside it (the same prompt, or an image and the edits made from it).

import type { ImageItem } from '../api/types';

export const SESSION_GAP_MS = 45 * 60 * 1000;

export interface Thread {
  key: string;
  /** The prompt that started it (an edit thread: the original's prompt). */
  prompt: string;
  /** Oldest first, the order they were made. */
  items: ImageItem[];
}

export interface Session {
  key: string;
  start: number;
  end: number;
  /** Newest thread first. */
  threads: Thread[];
  count: number;
}

const byTime = (a: ImageItem, b: ImageItem) => a.created_ms - b.created_ms;

/** The first image an edit chain started from, among the ones we have. */
export function rootOf(item: ImageItem, byId: Map<string, ImageItem>): ImageItem {
  let cur = item;
  const seen = new Set<string>();
  while (cur.parent_id && byId.has(cur.parent_id) && !seen.has(cur.id)) {
    seen.add(cur.id);
    cur = byId.get(cur.parent_id)!;
  }
  return cur;
}

/** "Edited from": oldest ancestor first, ending with the item itself. */
export function chainOf(item: ImageItem, items: ImageItem[]): ImageItem[] {
  const byId = new Map(items.map((i) => [i.id, i]));
  const chain = [item];
  const seen = new Set([item.id]);
  let cur = item;
  while (cur.parent_id && byId.has(cur.parent_id) && !seen.has(cur.parent_id)) {
    cur = byId.get(cur.parent_id)!;
    seen.add(cur.id);
    chain.unshift(cur);
  }
  return chain;
}

/** Edits made from this print. */
export const editsOf = (item: ImageItem, items: ImageItem[]) => items.filter((i) => i.parent_id === item.id).sort(byTime);

/** Sittings, newest first; inside each, threads newest first, prints oldest first. */
export function sessionsOf(items: ImageItem[], gap = SESSION_GAP_MS): Session[] {
  const sorted = [...items].sort(byTime);
  const byId = new Map(items.map((i) => [i.id, i]));
  const sittings: ImageItem[][] = [];
  for (const it of sorted) {
    const last = sittings.at(-1);
    if (last && it.created_ms - last[last.length - 1].created_ms <= gap) last.push(it);
    else sittings.push([it]);
  }
  return sittings
    .map((s) => {
      const threads = new Map<string, Thread>();
      for (const it of s) {
        // An edit joins its original's thread; prints with the same prompt share one.
        const root = rootOf(it, byId);
        const k = root.prompt.trim().toLowerCase();
        const t = threads.get(k) ?? { key: `${s[0].id}:${root.id}`, prompt: root.prompt, items: [] };
        t.items.push(it);
        threads.set(k, t);
      }
      const list = [...threads.values()].sort((a, b) => b.items[b.items.length - 1].created_ms - a.items[a.items.length - 1].created_ms);
      return { key: s[0].id, start: s[0].created_ms, end: s[s.length - 1].created_ms, threads: list, count: s.length };
    })
    .reverse();
}

/** "Today", "Yesterday", "Mon 16 Nov", with the year when it isn't this one. */
export function dayLabel(ms: number, now = Date.now()): string {
  const d = new Date(ms);
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diff = Math.round((day(new Date(now)) - day(d)) / 86400000);
  if (diff === 0) return 'Today';
  if (diff === 1) return 'Yesterday';
  return d.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short', ...(d.getFullYear() !== new Date(now).getFullYear() ? { year: 'numeric' } : {}) });
}

export const clock = (ms: number) => new Date(ms).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
