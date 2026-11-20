/**
 * Docs pages and their sidebar. The HTTP API reference is engine/API.md, split at build time into one
 * page per `##` section, so it can't drift from the engine.
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { Marked } from 'marked';

export interface Head { id: string; label: string }
export interface ApiSection { slug: string; title: string; html: string; heads: Head[] }

export const slug = (s: string) => s.toLowerCase().replace(/<[^>]+>|`/g, '').replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
const plain = (html: string) =>
  html.replace(/<[^>]+>/g, '').replace(/&amp;/g, '&').replace(/&#39;/g, "'").replace(/&quot;/g, '"').replace(/&lt;/g, '<').replace(/&gt;/g, '>');

/** Markdown with headings moved up one level (### → h2) and anchored; h2s are collected for "On this page". */
function render(md: string) {
  const heads: Head[] = [];
  const marked = new Marked({
    walkTokens(t) {
      if (t.type === 'heading') t.depth = Math.max(t.depth - 1, 2);
    },
    renderer: {
      heading({ tokens, depth }) {
        const text = this.parser.parseInline(tokens);
        const id = slug(text);
        if (depth === 2) heads.push({ id, label: plain(text) });
        return `<h${depth} id="${id}">${text}</h${depth}>\n`;
      },
    },
  });
  return { html: marked.parse(md) as string, heads };
}

let cache: { intro: string; sections: ApiSection[] } | null = null;
export function apiDocs() {
  if (cache) return cache;
  const md = readFileSync(resolve(process.cwd(), '..', 'engine', 'API.md'), 'utf8').replace(/^# .*\n/, '');
  const [introMd, ...chunks] = md.split(/^## /m);
  const sections = chunks.map((c) => {
    const nl = c.indexOf('\n');
    const titleMd = c.slice(0, nl).trim();
    const title = plain(new Marked().parseInline(titleMd) as string);
    return { slug: slug(titleMd), title, ...render(c.slice(nl + 1)) };
  });
  cache = { intro: new Marked().parse(introMd) as string, sections };
  return cache;
}

export interface NavLink { href: string; label: string }
export function docsNav(): { title: string; links: NavLink[] }[] {
  return [
    { title: 'Get started', links: [{ href: '/docs', label: 'Install' }] },
    {
      title: 'Studio',
      links: [
        { href: '/docs/create', label: 'Make and edit' },
        { href: '/docs/library', label: 'Library' },
        { href: '/docs/models', label: 'Models' },
        { href: '/docs/add-a-model', label: 'Add a model' },
        { href: '/docs/storage', label: 'Storage' },
        { href: '/docs/background', label: 'Tray and updates' },
      ],
    },
    { title: 'Use it', links: [{ href: '/docs/cli', label: 'CLI' }, { href: '/docs/mcp', label: 'MCP for agents' }] },
    {
      title: 'API reference',
      links: [{ href: '/docs/api', label: 'Overview' }, ...apiDocs().sections.map((s) => ({ href: `/docs/api/${s.slug}`, label: s.title }))],
    },
  ];
}
