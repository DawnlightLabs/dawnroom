/**
 * /registry.json for the engine: every manifest exactly as written in registry/*.toml (schema 2: shared files plus `variant`
 * blocks with each size tag's files and measurements), so each
 * model object deserialises into the engine's Manifest (deny_unknown_fields). Nothing extra inside.
 */
import type { APIRoute } from 'astro';
import { loadManifests } from '../lib/registry';

export const GET: APIRoute = () =>
  new Response(JSON.stringify({ schema: 2, models: loadManifests() }, null, 2) + '\n', {
    headers: { 'Content-Type': 'application/json' },
  });
