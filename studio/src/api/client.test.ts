import { describe, expect, it } from 'vitest';
import { createClient, EngineError, UNREACHABLE } from './client';
import type { JobEvent, PullEvent } from './types';

type Call = { url: string; init: RequestInit };

/** A client whose fetch records the request and answers with `respond`. */
function client(respond: (call: Call) => Response | Promise<Response>) {
  const calls: Call[] = [];
  const api = createClient('http://127.0.0.1:8420', async (url, init) => {
    const call = { url: String(url), init: init ?? {} };
    calls.push(call);
    return respond(call);
  });
  return { api, calls };
}

const jsonRes = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });
const sseRes = (text: string) => new Response(text, { headers: { 'Content-Type': 'text/event-stream' } });

describe('engine client', () => {
  it('sends JSON POSTs with the content type the engine insists on, even with no body', async () => {
    const { api, calls } = client(() => jsonRes({ id: 'j1' }, 202));
    await api.createJob({ prompt: 'a lighthouse', preset: 'fast', seed: 42 });
    await api.cancelJob('j1');
    expect(calls[0].url).toBe('http://127.0.0.1:8420/api/jobs');
    expect(calls[0].init.method).toBe('POST');
    expect(calls[0].init.headers).toEqual({ 'Content-Type': 'application/json' });
    expect(JSON.parse(calls[0].init.body as string)).toEqual({ prompt: 'a lighthouse', preset: 'fast', seed: 42 });
    expect(calls[1].init.headers).toEqual({ 'Content-Type': 'application/json' });
    expect(calls[1].init.body).toBeUndefined();
  });

  it("turns the engine's error shape into an EngineError", async () => {
    const { api } = client(() => jsonRes({ error: "Z-Image Turbo isn't downloaded yet. Pull z-image-turbo first.", code: 'model_not_installed' }, 409));
    const err = await api.createJob({ prompt: 'x' }).catch((e) => e);
    expect(err).toBeInstanceOf(EngineError);
    expect(err).toMatchObject({ code: 'model_not_installed', status: 409, message: "Z-Image Turbo isn't downloaded yet. Pull z-image-turbo first." });
  });

  it('reports an engine that is not running, and a non-JSON failure, in plain words', async () => {
    const down = client(() => {
      throw new TypeError('Failed to fetch');
    });
    await expect(down.api.health()).rejects.toMatchObject({ code: UNREACHABLE });
    const odd = client(() => new Response('bad gateway', { status: 502 }));
    await expect(odd.api.health()).rejects.toMatchObject({ code: 'http_error', status: 502 });
  });

  it('builds the library query from search and favourites', async () => {
    const { api, calls } = client(() => jsonRes({ items: [], total: 0, limit: 60, offset: 0 }));
    await api.images({ limit: 60, offset: 120, q: ' red door ', favourite: true });
    await api.images();
    expect(calls[0].url).toBe('http://127.0.0.1:8420/api/images?limit=60&offset=120&q=red+door&favourite=true');
    expect(calls[1].url).toBe('http://127.0.0.1:8420/api/images');
  });

  it('follows a job stream: snapshot, progress, preview, finished', async () => {
    const stream =
      'event: snapshot\ndata: {"id":"j1","status":"queued","progress":null}\n\n' +
      ': keep-alive\n\n' +
      'event: started\ndata: {"type":"started"}\n\n' +
      'event: progress\ndata: {"type":"progress","stage":"sampling","step":3,"total":8}\n\n' +
      'event: preview\ndata: {"type":"preview","step":3,"total":8,"b64_png":"AAAA"}\n\n' +
      'event: finished\ndata: {"type":"finished","job":{"id":"j1","status":"completed"}}\n\n';
    const { api, calls } = client(() => sseRes(stream));
    const events: JobEvent[] = [];
    await api.jobEvents('j1', (e) => events.push(e));
    expect(calls[0].url).toBe('http://127.0.0.1:8420/api/jobs/j1/events');
    expect(events.map((e) => e.type)).toEqual(['snapshot', 'started', 'progress', 'preview', 'finished']);
    expect(events[0]).toEqual({ type: 'snapshot', job: { id: 'j1', status: 'queued', progress: null } });
    expect(events[2]).toMatchObject({ stage: 'sampling', step: 3, total: 8 });
  });

  it('pull is a POST stream that ends with done or error', async () => {
    const stream =
      'event: progress\ndata: {"type":"progress","file":"vae/ae.safetensors","file_index":2,"file_count":3,"stage":"downloading","bytes":10,"total":100,"speed_bps":5}\n\n' +
      'event: error\ndata: {"type":"error","code":"cancelled","message":"The download was cancelled."}\n\n';
    const { api, calls } = client(() => sseRes(stream));
    const events: PullEvent[] = [];
    await api.pull('z-image-turbo', (e) => events.push(e));
    expect(calls[0].init.method).toBe('POST');
    expect(calls[0].init.headers).toEqual({ 'Content-Type': 'application/json' });
    expect(events.at(-1)).toEqual({ type: 'error', code: 'cancelled', message: 'The download was cancelled.' });
  });

  it('a refused stream rejects before any event', async () => {
    const { api } = client(() => jsonRes({ error: 'There\'s no model called "nope".', code: 'unknown_model' }, 400));
    await expect(api.pull('nope', () => {})).rejects.toMatchObject({ code: 'unknown_model' });
  });
});
