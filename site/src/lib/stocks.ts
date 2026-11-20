/**
 * Site-only presentation for each model: its box colour and plain-words uses. Facts (sizes, timings,
 * licences) stay in the registry manifests.
 */
export interface Stock {
  /** Band colour, from the Dawnlight palette. */
  colour: string;
  /** Text on the band: ink on terracotta (5.2:1), paper on blue (8.8:1) and sage (5.1:1). */
  text: string;
  /** What the box says on its band. */
  band: string;
  uses: string[];
  avoid: string[];
}

export const STOCKS: Record<string, Stock> = {
  'z-image-turbo': {
    colour: '#c47855',
    text: '#191B1D',
    band: 'Quality · 8 steps',
    uses: ['Posters, signs and labels: anything with words in it', 'The everyday all-rounder (the Quality preset)', 'Scenes and products that follow a long prompt closely'],
    avoid: ['When you want many quick drafts: Fast is about twice as quick'],
  },
  'flux2-klein-4b': {
    colour: '#0e4490',
    text: '#fcf8f5',
    band: 'Fast · 4 steps',
    uses: ['Quick drafts and lots of variations', 'Vivid product and scene shots', 'Agent loops: generate, look, refine'],
    avoid: ['Small text: letters can smear'],
  },
  'realvisxl-v5': {
    colour: '#59715c',
    text: '#fcf8f5',
    band: 'Photoreal · 28 steps',
    uses: ['Portraits: skin, faces and hair', 'Photoreal people and places'],
    avoid: ['Any text: it can’t write', 'Speed: 28 steps, slower than the turbo models'],
  },
};

export const stockOf = (id: string): Stock => STOCKS[id] ?? { colour: '#3a3d41', text: '#fcf8f5', band: 'Model', uses: [], avoid: [] };

/** Margin notes for people coming from ChatGPT or Gemini. */
export const NOTES = {
  vram: {
    term: 'VRAM',
    text: 'Video memory: the graphics card’s own working memory. A model has to fit in it while an image develops. ChatGPT and Gemini run on the company’s GPUs, so you never had to think about it.',
  },
  unified: {
    term: 'Unified memory',
    text: 'On Apple silicon and some AI PCs, the processor and graphics share one pool of memory. The GPU may use only part of it, so we count that part, not the whole number on the box.',
  },
  seed: {
    term: 'Seed',
    text: 'The number that starts the noise an image develops from. Same prompt, model, size and seed: the same picture, pixel for pixel, on the same machine. We checked: re-running frame 28 gave an identical image.',
  },
  gguf: {
    term: 'GGUF',
    text: 'A file format for compressed model weights. Q8 and Q4 mean 8 or 4 bits per weight: smaller downloads and less memory, a little detail traded away.',
  },
  mcp: {
    term: 'MCP',
    text: 'Model Context Protocol: the standard way agents like Claude Code and Codex use tools. Add the server once and your agent can make images on your GPU.',
  },
} as const;
