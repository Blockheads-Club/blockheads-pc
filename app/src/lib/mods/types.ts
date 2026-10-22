/** Shapes of the mod commands (`bhpc_launcher::mods`, serialised by serde as they are named in Rust). */

export interface ModError {
  code: string;
  detail?: string;
  file?: string;
  expected?: string;
  got?: string;
}

/** Statuses the launcher computes; `running` and `budget` come from the in-game loader (M6). */
export type ModStatus = 'off' | 'ready' | 'permissions' | 'dependency' | 'failed' | 'blocked' | 'running' | 'budget';

export interface DepInfo {
  id: string;
  req: string;
  installed: string | null;
  ok: boolean;
  soft: boolean;
  available: string | null;
}

/** One mod of the running game, as its mod loader reports it (`@mods` lines; bhpc_launcher::mods::live). */
export interface LiveMod {
  id: string;
  version: string;
  /** running | loaded | budget | failed */
  state: string;
  /** mine | dev | server */
  origin: string;
  /** Milliseconds of mod code in the last second. */
  ms: number;
  peak_ms: number;
  calls: number;
  reason?: string | null;
}

export interface ModSummary {
  id: string;
  name: Record<string, string>;
  description: Record<string, string>;
  version: string;
  authors: string[];
  license: string;
  homepage: string | null;
  sides: { server: boolean; client: boolean };
  runtime: string | null;
  api: string;
  native: boolean;
  permissions: string[];
  granted: string[];
  missing: string[];
  enabled: boolean;
  status: ModStatus;
  reason: string | null;
  detail: string | null;
  dependencies: DepInfo[];
  file: string;
  sha256: string;
  size: number;
  reviewed: boolean;
  update: string | null;
  update_asks_more: boolean;
  source: string;
  origin: string;
  installed_at: number;
  native_allowed_at: number | null;
}

export interface Overview {
  dir: string;
  size: number;
  mods: ModSummary[];
  problems: { file: string; message: string }[];
  error: ModError | null;
  attention: number;
  updates: number;
  loader_ready: boolean;
}

export interface BrowseMod {
  id: string;
  name: string;
  description: string | null;
  owners: string[];
  repo: string;
  tags: string[];
  version: string;
  compatible: boolean;
  compatible_version: string | null;
  sides: string[];
  permissions: string[];
  native: boolean;
  depends: Record<string, string>;
  size: number | null;
  published: string;
  versions: [string, string][];
  installed: string | null;
}

export interface Browse {
  generated: string;
  offline: boolean;
  error: ModError | null;
  mods: BrowseMod[];
  tags: string[];
}

export interface PlanItem {
  id: string;
  name: string;
  version: string;
  size: number | null;
  sides: string[];
  permissions: string[];
  native: boolean;
  needed_by: string | null;
  replaces: string | null;
  reviewed: boolean;
  authors: string[];
}

export interface Plan {
  items: PlanItem[];
  satisfied: { id: string; req: string; version: string }[];
  missing: { id: string; req: string; needed_by: string }[];
  total_bytes: number;
  token: string | null;
}

export interface InstallRequest {
  items: { id: string; version: string }[];
  token: string | null;
  grants: Record<string, string[]>;
  enable: boolean;
  native_confirmed: boolean;
}

export interface InstallProgress {
  stage: 'downloading' | 'checking' | 'writing';
  done: number;
  total: number;
}

export interface AutoUpdate {
  updated: [string, string][];
  failed: [string, ModError][];
}

export interface CachedMod {
  id: string;
  name: string;
  version: string;
  sha256: string;
  size: number;
  native: boolean;
  shared_with: string[];
}

export interface ServerCache {
  key: string;
  name: string;
  address: string;
  last_played: number;
  size: number;
  mods: CachedMod[];
}

export interface CacheOverview {
  servers: ServerCache[];
  total: number;
}

export interface DevMod {
  path: string;
  id: string | null;
  version: string | null;
  name: string | null;
  permissions: string[];
  native: boolean;
  errors: string[];
  warnings: string[];
}
