// The one typed client of the engine's HTTP API (engine/API.md).

import { readSse } from './sse';
import type { CheckReport, LocalScan, AutoAnswer, DrivesAnswer, StoredInput, Hardware, Health, MoveProgress, StorageReport, ImagePage, Info, Job, JobEvent, JobRequest, JobView, Model, ModelList, PresetList, PullEvent } from './types';

/** An engine refusal: `message` is plain words ending in what to do next; branch on `code`. */
export class EngineError extends Error {
  constructor(
    message: string,
    readonly code: string,
    readonly status: number,
  ) {
    super(message);
    this.name = 'EngineError';
  }
}

export const UNREACHABLE = 'engine_unreachable';

export function isAbort(e: unknown): boolean {
  return e instanceof DOMException && e.name === 'AbortError';
}

type Fetch = typeof fetch;

export function createClient(base: string, fetchFn: Fetch = (...a) => fetch(...a)) {
  /** One request. Every POST carries `Content-Type: application/json`, even with no body (the engine requires it). */
  async function send(method: string, path: string, body?: unknown, signal?: AbortSignal): Promise<Response> {
    let res: Response;
    try {
      res = await fetchFn(base + path, {
        method,
        signal,
        headers: method === 'POST' ? { 'Content-Type': 'application/json' } : undefined,
        body: body === undefined ? undefined : JSON.stringify(body),
      });
    } catch (e) {
      if (isAbort(e)) throw e;
      throw new EngineError("Can't reach the engine. Check that it's running, then try again.", UNREACHABLE, 0);
    }
    if (res.ok) return res;
    let payload: { error?: unknown; code?: unknown } = {};
    try {
      payload = await res.json();
    } catch {
      // not JSON: fall back to the status below
    }
    const message = typeof payload.error === 'string' ? payload.error : `The engine answered ${res.status}. Try again.`;
    throw new EngineError(message, typeof payload.code === 'string' ? payload.code : 'http_error', res.status);
  }

  const json = async <T>(method: string, path: string, body?: unknown, signal?: AbortSignal) => (await send(method, path, body, signal)).json() as Promise<T>;

  return {
    base,
    health: (signal?: AbortSignal) => json<Health>('GET', '/api/health', undefined, signal),
    info: () => json<Info>('GET', '/api/info'),
    hardware: () => json<Hardware>('GET', '/api/hardware'),
    storage: () => json<StorageReport>('GET', '/api/storage'),
    clearCache: () => json<{ freed_bytes: number; removed: string[] }>('POST', '/api/storage/clear-cache'),
    deletePartials: (body: { file: string } | { all: true }) => json<{ deleted: string[]; freed_bytes: number }>('POST', '/api/storage/partials/delete', body),
    startMove: (to: string) => json<MoveProgress>('POST', '/api/storage/move', { to }),
    moveStatus: () => json<MoveProgress>('GET', '/api/storage/move'),
    drives: () => json<DrivesAnswer>('GET', '/api/storage/drives'),
    models: (signal?: AbortSignal) => json<ModelList>('GET', '/api/models', undefined, signal),
    presets: () => json<PresetList>('GET', '/api/presets'),
    auto: (signal?: AbortSignal) => json<AutoAnswer>('GET', '/api/auto', undefined, signal),
    /** Stores an edit input: a file on this computer (`path`) or the bytes (`b64`, a data: URL is fine). */
    storeInput: (body: { path: string } | { b64: string }) => json<StoredInput>('POST', '/api/inputs', body),

    /** The custom-model check: a Hugging Face source or a file on this computer. Downloads no weights. */
    checkCustom: (body: { source: string } | { path: string }) => json<CheckReport>('POST', '/api/models/custom/check', body),
    /** Imports a checked local file (hard link or copy); the pull that follows runs the test image. */
    importCustom: (path: string) => json<Model>('POST', '/api/models/custom/import', { path }),
    /** Image models already on this computer: the usual folders, or only `dir`. Headers only. */
    findLocal: (dir?: string) => json<LocalScan>('POST', '/api/models/custom/local', dir ? { dir } : {}),
    createJob: (req: JobRequest) => json<Job>('POST', '/api/jobs', req),
    job: (id: string) => json<JobView>('GET', `/api/jobs/${id}`),
    cancelJob: (id: string) => json<{ id: string; cancel_requested: boolean }>('POST', `/api/jobs/${id}/cancel`),

    /** Follows one job until it finishes. Resolves when the stream closes; abort to detach. */
    async jobEvents(id: string, onEvent: (e: JobEvent) => void, signal?: AbortSignal): Promise<void> {
      const res = await send('GET', `/api/jobs/${id}/events`, undefined, signal);
      await readSse(res, (m) => {
        const data = JSON.parse(m.data);
        // The snapshot is the bare job view; the other events already carry their `type`.
        onEvent(m.event === 'snapshot' ? { type: 'snapshot', job: data } : data);
      });
    },

    images(opts: { limit?: number; offset?: number; q?: string; favourite?: boolean } = {}, signal?: AbortSignal) {
      const p = new URLSearchParams();
      if (opts.limit) p.set('limit', String(opts.limit));
      if (opts.offset) p.set('offset', String(opts.offset));
      if (opts.q?.trim()) p.set('q', opts.q.trim());
      if (opts.favourite) p.set('favourite', 'true');
      const qs = p.toString();
      return json<ImagePage>('GET', `/api/images${qs ? `?${qs}` : ''}`, undefined, signal);
    },
    /** The PNG, or with `w` a JPEG thumbnail no larger than that on its longest side. */
    imageUrl: (id: string, w?: number) => `${base}/api/images/${id}/file${w ? `?w=${w}` : ''}`,
    setFavourite: (id: string, favourite: boolean) => json<{ id: string; favourite: boolean }>('POST', `/api/images/${id}/favourite`, { favourite }),
    deleteImage: (id: string) => json<{ id: string; deleted: boolean }>('DELETE', `/api/images/${id}`),

    /**
     * Starts a pull, or joins the one already running. The engine owns the download: aborting
     * `signal` only detaches. Resolves after the `done` or `error` event.
     */
    async pull(id: string, onEvent: (e: PullEvent) => void, signal?: AbortSignal): Promise<void> {
      const res = await send('POST', `/api/models/${encodeURIComponent(id)}/pull`, undefined, signal);
      await readSse(res, (m) => onEvent(JSON.parse(m.data)));
    },
    /** The user accepted a gated model's licence (only after they clicked Accept in the sheet). */
    acceptLicence: (id: string) => json<{ model: string; licence_url: string; accepted_ms: number }>('POST', `/api/models/${encodeURIComponent(id)}/licence/accept`),
    cancelPull: (id: string) => json<{ id: string; cancel_requested: boolean }>('POST', `/api/models/${encodeURIComponent(id)}/pull/cancel`),
    deleteModel: (id: string) => json<{ id: string; deleted: string[]; kept: string[] }>('DELETE', `/api/models/${encodeURIComponent(id)}`),
  };
}

export type Client = ReturnType<typeof createClient>;
