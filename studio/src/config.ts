/** The product name, for Studio (the engine, site and Tauri config keep their own copies). */
export const PRODUCT_NAME = 'Dawnroom';
export const STUDIO_VERSION = '0.1.0';
export const LICENCE = 'Apache-2.0';

/** The site. Dev builds open the local site preview (`npm run preview` in site/, port 4322). */
export const SITE_URL = import.meta.env?.DEV ? 'http://localhost:4322' : 'https://dawnroom.dawnlightlabs.com';
/** The guide the "How adding a model works" section opens. */
export const DOCS_ADD_MODEL = `${SITE_URL}/docs/add-a-model`;
/** Where each "How this works" link goes: a docs page or one of its anchors (the site's link check covers them). */
export const DOCS = {
  create: `${SITE_URL}/docs/create`,
  auto: `${SITE_URL}/docs/create#auto`,
  edit: `${SITE_URL}/docs/create#edit`,
  library: `${SITE_URL}/docs/library`,
  models: `${SITE_URL}/docs/models`,
  sizeTags: `${SITE_URL}/docs/models#sizes`,
  offload: `${SITE_URL}/docs/models#memory`,
  licences: `${SITE_URL}/docs/models#licences`,
  storage: `${SITE_URL}/docs/storage`,
  move: `${SITE_URL}/docs/storage#move`,
  agents: `${SITE_URL}/docs/mcp`,
  tray: `${SITE_URL}/docs/background#tray`,
  updates: `${SITE_URL}/docs/background#updates`,
  addModel: DOCS_ADD_MODEL,
} as const;

const DEFAULT_ENGINE = 'http://127.0.0.1:8420';

/**
 * Where the engine listens. Dev can point Studio at another local engine with
 * `VITE_ENGINE_URL` or `?engine=http://127.0.0.1:8421`; only loopback addresses are accepted.
 */
export function engineUrl(search: string = globalThis.location?.search ?? '', env: string | undefined = import.meta.env?.VITE_ENGINE_URL): string {
  const wanted = new URLSearchParams(search).get('engine') ?? env;
  if (!wanted) return DEFAULT_ENGINE;
  try {
    const u = new URL(wanted);
    if (u.protocol === 'http:' && (u.hostname === '127.0.0.1' || u.hostname === 'localhost')) return u.origin;
  } catch {
    // fall through to the default
  }
  return DEFAULT_ENGINE;
}

export const ENGINE_URL = engineUrl();
export const ENGINE_PORT = Number(new URL(ENGINE_URL).port || 80);
