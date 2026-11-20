/**
 * Versions shown on /download, newest first. Empty until v0.1 ships: add an entry per release with its
 * real notes and files (size and sha256 from the built artifact, never typed from memory).
 */
export interface ReleaseFile {
  platform: 'windows' | 'linux' | 'macos';
  /** What it is, e.g. "Installer (x64)". */
  label: string;
  url: string;
  bytes: number;
  sha256: string;
}
export interface Release {
  version: string;
  /** ISO date, e.g. "2026-12-01". */
  date: string;
  /** One line: what this release is about. */
  summary: string;
  notes: string[];
  files: ReleaseFile[];
}

export const RELEASES: Release[] = [
  {
    version: '0.1.0',
    date: '2026-11-20',
    summary: 'The first release: local image generation and editing, with Studio, a CLI and an MCP server.',
    notes: [
      'Generate and edit images with 34 open models; Auto picks the model for you.',
      'Custom models from Hugging Face or your disk, checked before anything is downloaded.',
      'Windows: NVIDIA through CUDA; AMD, Intel Arc and built-in graphics through Vulkan (experimental).',
      'Linux is experimental and needs glibc 2.43 or newer (Ubuntu 26.04, current Fedora or Arch).',
    ],
    files: [
      { platform: 'windows', label: 'Installer (x64)', url: 'https://github.com/DawnlightLabs/dawnroom/releases/download/v0.1.0/Dawnroom_0.1.0_x64-setup.exe', bytes: 7826573, sha256: '88556b89438a0129090db8ee31c826a0a51085aad080410d33960c0dd712cc96' },
      { platform: 'windows', label: 'CLI and engine (zip)', url: 'https://github.com/DawnlightLabs/dawnroom/releases/download/v0.1.0/dawnroom-0.1.0-windows-x64.zip', bytes: 5769237, sha256: 'aa6733180674b22d749f3d6a8830e3e2590146a6979f2dbce2765125d3f95d53' },
      { platform: 'linux', label: 'AppImage (Studio + engine)', url: 'https://github.com/DawnlightLabs/dawnroom/releases/download/v0.1.0/Dawnroom_0.1.0_amd64.AppImage', bytes: 92965368, sha256: 'fa3a1e5edaa0b01974316370c98c519c94cb19e87adb923379310c31a1410ec9' },
      { platform: 'linux', label: '.deb (Studio + engine)', url: 'https://github.com/DawnlightLabs/dawnroom/releases/download/v0.1.0/Dawnroom_0.1.0_amd64.deb', bytes: 13564176, sha256: '4373e90443721062abd1ff478355d46e863421f361a9b942ed8d7d60f0f68246' },
      { platform: 'linux', label: 'CLI and engine (tar.gz)', url: 'https://github.com/DawnlightLabs/dawnroom/releases/download/v0.1.0/dawnroom-0.1.0-linux-x64.tar.gz', bytes: 6623001, sha256: '3be634a58ad99f7210c261877f74101897a291cb15cfd895df5bbd3cbadb9340' },
    ],
  },
];
