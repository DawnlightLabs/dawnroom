import conf from '../../src-tauri/tauri.conf.json';
import { describe, expect, it } from 'vitest';
import { STUDIO_VERSION } from '../config';
import { restartBlocker, trayLines } from './background';

describe('updates', () => {
  it('Studio version matches the desktop app (the updater compares against it)', () => {
    expect(conf.version).toBe(STUDIO_VERSION);
  });
  it('never restarts to update mid-job or mid-download', () => {
    const idle = { models: [], pulls: {}, health: { busy: false }, groups: [] };
    expect(restartBlocker(idle as never)).toBeNull();
    expect(restartBlocker({ ...idle, health: { busy: true } } as never)).toMatch(/develops/);
    expect(restartBlocker({ ...idle, groups: [{ prints: [{ status: 'queued' }] }] } as never)).toMatch(/develops/);
    expect(restartBlocker({ ...idle, models: [{ id: 'z', state: 'pulling' }] } as never)).toMatch(/download/);
  });
});

describe('tray lines', () => {
  const model = { id: 'z', name: 'Z-Image Turbo', state: 'pulling', size_bytes: 100, files: [{ path: 'a', size: 100 }], pull: null };
  it('lists downloads and the GPU state', () => {
    const s = { models: [model], pulls: { z: { stage: 'downloading', file: 'a', bytes: 62, total: 100, speed_bps: 1 } }, health: { busy: false, model: null }, conn: 'ok' } as never;
    expect(trayLines(s)).toEqual({ downloads: ['Z-Image Turbo · 62%'], gpu: 'GPU idle' });
  });
  it('says Developing and what is loaded', () => {
    const base = { models: [{ ...model, state: 'installed' }], pulls: {}, conn: 'ok' };
    expect(trayLines({ ...base, health: { busy: true, model: 'z' } } as never).gpu).toBe('Developing…');
    expect(trayLines({ ...base, health: { busy: false, model: 'z' } } as never).gpu).toBe('Z-Image Turbo loaded');
    expect(trayLines({ ...base, health: null, conn: 'down' } as never)).toEqual({ downloads: [], gpu: null });
  });
});
