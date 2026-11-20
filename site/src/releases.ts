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

export const RELEASES: Release[] = [];
