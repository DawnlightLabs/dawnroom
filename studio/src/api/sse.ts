// Server-sent events over fetch. EventSource can't POST (the pull endpoint is a POST stream)
// and can't be aborted cleanly, so both engine streams go through this one reader.

export interface SseMessage {
  event: string;
  data: string;
}

/**
 * Incremental SSE parser. Feed it text as it arrives, in chunks split anywhere; it calls
 * `onMessage` once per complete event. Handles LF and CRLF, `:` keep-alive comments and
 * multi-line `data:` fields.
 */
export function createSseParser(onMessage: (m: SseMessage) => void) {
  let buffer = '';
  let event = '';
  let data: string[] = [];

  const line = (text: string) => {
    if (text === '') {
      if (data.length) onMessage({ event: event || 'message', data: data.join('\n') });
      event = '';
      data = [];
      return;
    }
    if (text.startsWith(':')) return;
    const colon = text.indexOf(':');
    const field = colon === -1 ? text : text.slice(0, colon);
    let value = colon === -1 ? '' : text.slice(colon + 1);
    if (value.startsWith(' ')) value = value.slice(1);
    if (field === 'event') event = value;
    else if (field === 'data') data.push(value);
  };

  return {
    feed(chunk: string) {
      buffer += chunk;
      for (;;) {
        const end = buffer.indexOf('\n');
        if (end === -1) return;
        // A lone "\r" at the end of the buffer may be half of a CRLF; it stays buffered until the "\n".
        line(buffer.slice(0, end).replace(/\r$/, ''));
        buffer = buffer.slice(end + 1);
      }
    },
  };
}

/**
 * Reads an SSE response to its end, calling `onMessage` for each event. Resolves when the
 * server closes the stream; rejects with an AbortError when `signal` aborts (that is how a
 * client detaches).
 */
export async function readSse(response: Response, onMessage: (m: SseMessage) => void): Promise<void> {
  if (!response.body) throw new Error('The response has no body to stream.');
  const parser = createSseParser(onMessage);
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    parser.feed(decoder.decode(value, { stream: true }));
  }
  parser.feed(decoder.decode());
}
