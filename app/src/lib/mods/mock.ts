import { vi } from 'vitest';
import type { Browse, BrowseMod, CacheOverview, DevMod, ModSummary, Overview, Plan, PlanItem } from './types';

/** Test data for the mod screens (shapes of `bhpc_launcher::mods`). */
export function makeMod(over: Partial<ModSummary> = {}): ModSummary {
  return {
    id: 'minimap',
    name: { en: 'Minimap', tr: 'Mini harita' },
    description: { en: 'A small map in the corner.' },
    version: '0.9.1',
    authors: ['pixelpan'],
    license: 'MIT',
    homepage: 'https://github.com/pixelpan/minimap',
    sides: { server: true, client: true },
    runtime: 'lua',
    api: '^0.1',
    native: false,
    permissions: ['world:read', 'channels'],
    granted: ['world:read', 'channels'],
    missing: [],
    enabled: true,
    status: 'ready',
    reason: null,
    detail: null,
    dependencies: [],
    file: 'minimap-0.9.1.bhmod',
    sha256: 'ab'.repeat(32),
    size: 420_000,
    reviewed: true,
    update: null,
    update_asks_more: false,
    source: 'index',
    origin: 'https://m.test/minimap-0.9.1.bhmod',
    installed_at: 1_759_449_600,
    native_allowed_at: null,
    ...over
  };
}

export function makeOverview(over: Partial<Overview> = {}): Overview {
  const mods = over.mods ?? [
    makeMod(),
    makeMod({ id: 'better-sounds', name: { en: 'Better Sounds' }, version: '1.3.0', permissions: [], granted: [], sides: { server: false, client: true }, update: '1.4.0' }),
    makeMod({ id: 'zoom-out', name: { en: 'Zoom Out' }, version: '2.0.0', enabled: false, status: 'off', sides: { server: false, client: true } }),
    makeMod({
      id: 'day-sky', name: { en: 'Day & Night Sky' }, version: '1.0.0', status: 'dependency', detail: 'sky-core ^1',
      dependencies: [{ id: 'sky-core', req: '^1', installed: null, ok: false, soft: false, available: '1.2.0' }]
    }),
    makeMod({
      id: 'pixel-shaders', name: { en: 'Pixel Shaders' }, version: '0.7.2', native: true, permissions: ['native'], granted: ['native'],
      reviewed: false, source: 'file', origin: 'pixel-shaders.bhmod', native_allowed_at: 1_760_227_200, license: '', homepage: null, authors: []
    })
  ];
  return {
    dir: '/data/mods', size: 3_000_000, mods, problems: [], error: null,
    attention: mods.filter((m) => ['failed', 'blocked', 'dependency', 'permissions'].includes(m.status)).length,
    updates: mods.filter((m) => m.update).length, loader_ready: false, ...over
  };
}

export function makeBrowseMod(over: Partial<BrowseMod> = {}): BrowseMod {
  return {
    id: 'sky-core', name: 'Sky Core', description: 'Shared sky and weather code other mods use.', owners: ['lumen'],
    repo: 'https://github.com/lumen/sky-core', tags: ['library'], version: '1.2.0', compatible: true, compatible_version: '1.2.0',
    sides: ['client'], permissions: [], native: false, depends: {}, size: 210_000, published: '2026-10-01',
    versions: [['1.2.0', '2026-10-01'], ['1.1.0', '2026-09-01']], installed: null, ...over
  };
}

export function makeBrowse(over: Partial<Browse> = {}): Browse {
  return {
    generated: '2026-10-21T12:00:00Z', offline: false, error: null, tags: ['fun', 'graphics', 'library'],
    mods: [
      makeBrowseMod(),
      makeBrowseMod({ id: 'pets', name: 'Pet Companions', owners: ['pixelpan'], tags: ['fun'], sides: ['server', 'client'], permissions: ['world:read', 'channels'], description: 'Tame a dodo.' }),
      makeBrowseMod({ id: 'turbo', name: 'Turbo Lighting', tags: ['graphics'], native: true, permissions: ['native'], description: null }),
      makeBrowseMod({ id: 'minimap', name: 'Minimap', tags: [], installed: '0.9.1', version: '0.9.1' }),
      makeBrowseMod({ id: 'better-sounds', name: 'Better Sounds', installed: '1.3.0', version: '1.4.0' }),
      makeBrowseMod({ id: 'admin', name: 'Admin Tools', sides: ['server'], tags: [] }),
      makeBrowseMod({ id: 'keys', name: 'Key Hints', compatible: false, compatible_version: '1.6.0', version: '2.0.0' })
    ],
    ...over
  };
}

export function makePlanItem(over: Partial<PlanItem> = {}): PlanItem {
  return {
    id: 'day-sky', name: 'Day & Night Sky', version: '1.0.0', size: 640_000, sides: ['client'], permissions: ['world:read'],
    native: false, needed_by: null, replaces: null, reviewed: true, authors: ['lumen'], ...over
  };
}

export function makePlan(over: Partial<Plan> = {}): Plan {
  return {
    items: [makePlanItem(), makePlanItem({ id: 'sky-core', name: 'Sky Core', version: '1.2.0', size: 210_000, permissions: [], needed_by: 'day-sky' })],
    satisfied: [], missing: [], total_bytes: 850_000, token: null, ...over
  };
}

export function makeServers(over: Partial<CacheOverview> = {}): CacheOverview {
  return {
    total: 5_100_000,
    servers: [
      {
        key: 'mira.example.net_15151', name: 'Sunny Isle', address: 'mira.example.net:15151', last_played: 1_761_100_000, size: 3_900_000,
        mods: [
          { id: 'minimap', name: 'Minimap', version: '0.9.1', sha256: 'aa', size: 420_000, native: false, shared_with: ['Dodo Land'] },
          { id: 'turbo', name: 'Turbo Lighting', version: '2.0.1', sha256: 'bb', size: 2_400_000, native: true, shared_with: [] }
        ]
      },
      {
        key: '192.168.1.20_15151', name: 'Dodo Land', address: '192.168.1.20:15151', last_played: 1_759_900_000, size: 1_200_000,
        mods: [{ id: 'minimap', name: 'Minimap', version: '0.9.1', sha256: 'aa', size: 420_000, native: false, shared_with: ['Sunny Isle'] }]
      }
    ],
    ...over
  };
}

export function makeDevMod(over: Partial<DevMod> = {}): DevMod {
  return { path: '/code/my-mod', id: 'my-mod', version: '0.1.0-dev', name: 'My mod', permissions: [], native: false, errors: [], warnings: [], ...over };
}

/** The mod commands of the fake `api`. */
export function makeModsApi() {
  return {
    modsOverview: vi.fn(async () => makeOverview()),
    modsIndex: vi.fn(async () => makeBrowse()),
    modsPlan: vi.fn(async () => makePlan()),
    modsInspectFile: vi.fn(async () => makePlan({ items: [makePlanItem({ id: 'pixel-shaders', name: 'Pixel Shaders', reviewed: false, permissions: [] })], token: 'tok' })),
    modsInspectLink: vi.fn(async () => makePlan({ items: [makePlanItem({ id: 'pixel-shaders', name: 'Pixel Shaders', reviewed: false, permissions: [] })], token: 'tok' })),
    modsInstall: vi.fn(async () => ['day-sky']),
    onModsInstall: vi.fn(async () => () => {}),
    modsSetEnabled: vi.fn(async () => {}),
    modsSetGrant: vi.fn(async () => {}),
    modsRemove: vi.fn(async () => {}),
    modsAutoUpdate: vi.fn(async () => ({ updated: [], failed: [] })),
    modsServers: vi.fn(async () => makeServers()),
    modsClearServer: vi.fn(async () => makeServers({ servers: [] , total: 0 })),
    modsClearServers: vi.fn(async () => makeServers({ servers: [], total: 0 })),
    modsDevInspect: vi.fn(async (path: string) => makeDevMod({ path })),
    modsDevReload: vi.fn(async () => {
      throw { code: 'needsLoader' };
    })
  };
}
