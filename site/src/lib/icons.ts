/**
 * The site's one icon set: 24 grid, 1.5 px stroke (kept at 1.5 px at any size), square caps, mitred
 * joins. Paths come from the Dawnlight design system's icon set where it has the glyph (check, search, download,
 * close, chevrons, arrow, external, settings/filter, terminal, circuit/chip), else Studio's (ui.tsx).
 * A path after "|" is a dot, drawn heavier with round caps so it survives at 14 px.
 * Rendered once as an inline sprite (Icons.astro); pages and client scripts use <use href="#i-…">.
 * No Node imports: client scripts import icon() too.
 */
export const ICONS = {
  // Studio's
  copy: 'M8 8h12v12H8zM4 4h12v4M4 4v12h4',
  check: 'M5 12.5l4.5 4.5L19 7.5',
  search: 'M4.5 10.5a6 6 0 1 0 12 0a6 6 0 1 0-12 0M15 15l5 5',
  download: 'M12 4v11M7 10l5 5 5-5M5 20h14',
  close: 'M6 6l12 12M18 6L6 18',
  external: 'M13 4h7v7M20 4l-9 9M17 14v6H4V7h6',
  back: 'M15 5l-7 7 7 7',
  next: 'M9 5l7 7-7 7',
  down: 'M5 9l7 7 7-7',
  arrow: 'M4 12h15M13 6l6 6-6 6',
  filter: 'M4 7h9M17 7h3M4 17h3M11 17h9M13 7a2 2 0 1 0 4 0a2 2 0 1 0-4 0M7 17a2 2 0 1 0 4 0a2 2 0 1 0-4 0',
  retry: 'M5 12a7 7 0 1 1 2.5 5.4M5 18v-5h5',
  // Darkroom glyphs
  loupe: 'M10 4a6 6 0 1 0 0 12 6 6 0 0 0 0-12zM14.5 14.5L20 20M7 10a3 3 0 0 1 3-3',
  reel: 'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18z|M12 7.5h.01M12 16.5h.01M7.5 12h.01M16.5 12h.01M12 12h.01',
  timer: 'M12 7a7 7 0 1 0 0 14 7 7 0 0 0 0-14zM12 10.5V14M9.5 3h5M12 3v4M18 6.5l1.5-1.5',
  tray: 'M3 10h18l-2 9H5zM7 14.5c1.6-1 3.4 1 5 0s3.4-1 5 0',
  terminal: 'M3 5h18v14H3zM7 10l3 2-3 2M12 15h5',
  chip: 'M4 12h4M16 12h4M12 4v4M12 16v4M8 8h8v8H8z|M12 12h.01',
  plug: 'M8 3v5M16 3v5M6 8h12v4a6 6 0 0 1-12 0zM12 18v3',
  pick: 'M5 13c-1-5 3-9 8-9 4.5 0 7 3 6.5 7.5-.6 5-4.6 8-9 7.5C6.5 18.6 4 16 4.3 12.5',
} as const;

export type IconName = keyof typeof ICONS;

/** Markup for client scripts that build HTML strings. Decorative: the control names itself. */
export const icon = (name: IconName, cls = '') => `<svg class="i ${cls}" aria-hidden="true" focusable="false"><use href="#i-${name}"/></svg>`;
