import { describe, expect, it } from 'vitest';
import { createSseParser, readSse, type SseMessage } from './sse';

function parse(chunks: string[]): SseMessage[] {
  const out: SseMessage[] = [];
  const p = createSseParser((m) => out.push(m));
  for (const c of chunks) p.feed(c);
  return out;
}

const STREAM = 'event: snapshot\ndata: {"id":"a"}\n\nevent: progress\ndata: {"type":"progress","step":3}\n\n';

describe('createSseParser', () => {
  it('reads named events', () => {
    expect(parse([STREAM])).toEqual([
      { event: 'snapshot', data: '{"id":"a"}' },
      { event: 'progress', data: '{"type":"progress","step":3}' },
    ]);
  });

  it('gives the same events however the bytes are split', () => {
    const whole = parse([STREAM]);
    for (let size = 1; size < STREAM.length; size++) {
      const chunks = STREAM.match(new RegExp(`[\\s\\S]{1,${size}}`, 'g'))!;
      expect(parse(chunks), `chunk size ${size}`).toEqual(whole);
    }
  });

  it('handles CRLF line endings, even split between the CR and the LF', () => {
    const crlf = STREAM.replaceAll('\n', '\r\n');
    expect(parse([crlf])).toEqual(parse([STREAM]));
    const cut = crlf.indexOf('\r\n') + 1;
    expect(parse([crlf.slice(0, cut), crlf.slice(cut)])).toEqual(parse([STREAM]));
  });

  it('skips keep-alive comments and waits for the blank line', () => {
    expect(parse([':\n\n', ': ping\n', 'event: done\ndata: {}\n'])).toEqual([]);
    expect(parse([':\n\n', 'event: done\n', ': ping\n', 'data: {}\n\n'])).toEqual([{ event: 'done', data: '{}' }]);
  });

  it('joins multi-line data and defaults the event name', () => {
    expect(parse(['data: one\ndata: two\n\n'])).toEqual([{ event: 'message', data: 'one\ntwo' }]);
    expect(parse(['data:no-space\n\n'])).toEqual([{ event: 'message', data: 'no-space' }]);
  });
});

describe('readSse', () => {
  it('decodes a byte stream, including a multi-byte character split across chunks', async () => {
    const bytes = new TextEncoder().encode('event: progress\ndata: {"file":"café.gguf"}\n\n');
    const cut = bytes.indexOf(0xc3) + 1; // between the two bytes of "é"
    const body = new ReadableStream<Uint8Array>({
      start(c) {
        c.enqueue(bytes.slice(0, cut));
        c.enqueue(bytes.slice(cut));
        c.close();
      },
    });
    const out: SseMessage[] = [];
    await readSse(new Response(body), (m) => out.push(m));
    expect(out).toEqual([{ event: 'progress', data: '{"file":"café.gguf"}' }]);
  });
});
