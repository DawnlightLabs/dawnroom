/**
 * Everything about the product, and what isn't live yet, in one place.
 * Studio, the engine and Tauri keep their own copies of the product name.
 */
export const PRODUCT_NAME = 'Dawnroom';
export const BIN = 'dawnroom';
export const VERSION = '0.1.0';
export const SITE_URL = 'https://dawnroom.dawnlightlabs.com';
/** One-command installs (site/public/install.ps1 and install.sh); `--cli` = engine + CLI only. */
export const INSTALL_PS1 = `irm ${SITE_URL}/install.ps1 | iex`;
export const INSTALL_SH = `curl -fsSL ${SITE_URL}/install.sh | sh`;
export const ORG = 'Dawnlight Labs';
export const LICENCE = 'Apache-2.0';

/** Planned public repo; not public yet. */
export const REPO_URL = 'https://github.com/DawnlightLabs/dawnroom';

/** The Windows installer ships with the v0.1 release. Until it exists this stays null and the site says so. */
export const DOWNLOAD_URL: string | null = 'https://github.com/DawnlightLabs/dawnroom/releases/download/v0.1.0/Dawnroom_0.1.0_x64-setup.exe';
export const DOWNLOAD_URL_LINUX: string | null = 'https://github.com/DawnlightLabs/dawnroom/releases/download/v0.1.0/Dawnroom_0.1.0_amd64.AppImage';

export const ENGINE_PORT = 8420;

/** The name agents register the MCP server under. */
export const MCP_NAME = 'dawnroom';

/**
 * Launch videos, made in the clean-install + media pass from real footage. Until then they stay null and
 * their slots (data-slot="trailer" on the landing, data-slot="demo" on /docs) render nothing visible.
 */
export const TRAILER_URL: string | null = '/video/trailer-1080.mp4';
export const TRAILER_POSTER = '/video/trailer-poster.jpg';
export const DEMO_URL: string | null = null;
