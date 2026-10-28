import type { Page } from '@playwright/test';

/**
 * Replaces the Tauri IPC with an in-page fake launcher backend so the built web UI can be driven in a browser.
 * Scenarios come from the URL query: ?first=1 (no saved config), ?os=macos|windows|linux, ?game=ready|running|
 * stopped|crashed|missing, ?lang=en|tr|ru|kk (saved language), ?fresh=1 (no saved preferences: the language follows
 * the browser's), ?mods=empty|error (Mods area), ?modsindex=offline|slow, ?dev=1 (mods developer mode), ?updates=1 (the update feed exists), ?update=available, ?splash=hang|error (main window boot) and, on /splash, ?splash=slow|error&first=1 (state of the splash window).
 */
export async function mockTauri(page: Page) {
  await page.addInitScript(() => {
    const q = new URLSearchParams(location.search);
    const os = q.get('os') ?? 'linux';
    const defaults = {
      apk: '',
      data_dir: '/home/mira/.local/share/blockheads-pc',
      force: false,
      window: { width: 1280, height: 720, fullscreen: false, vsync: true, ui_scale: 1, show_fps: false, fps_position: 'top-left', zoom: 'phone', fps_limit: 60, game_speed: 1, parallel_threads: true },
      keys: { touch: 'Left', pan: 'Right', zoom: 'Wheel', pinch: 'Ctrl+Left', back: 'Esc', fullscreen: 'F11', hide_ui: 'H', chat: 'T', voice_proximity: 'V', voice_channel: 'B', voice_global: 'G', voice_broadcast: 'J' },
      controller: true,
      show_dpad: false,
      pad_tile_cursor: false
    };
    const saved = {
      ...defaults,
      apk: '/home/mira/.local/share/blockheads-pc/game/TheBlockheads.apk',
      window: { width: 1600, height: 900, fullscreen: false, vsync: true, ui_scale: 1.25, show_fps: false, fps_position: 'top-left', zoom: 'phone', fps_limit: 60, game_speed: 1, parallel_threads: true }
    };
    const s: any = {
      config: q.get('first') ? null : structuredClone(saved),
      prefs: {
        language: q.get('lang') ?? 'en', theme: 'system', auto_update_check: true, keep_private_copy: true, game_sound: true, mute_in_background: false,
        mods_auto_update: true, mods_cache_limit_mb: 1024, mods_block_native: false, mods_developer: !!q.get('dev'), mods_dev_folders: q.get('dev') ? ['/home/mira/code/my-mod'] : [], mods_dev_watch: true
      },
      game: q.get('game') ?? 'ready',
      startedAt: 0,
      lines: [] as string[],
      lastSecs: 3840,
      exit: null as number | null,
      pick: '/home/mira/Downloads/TheBlockheads.apk',
      updateStage: 'idle',
      installStart: 0,
      missingApk: false,
      bootFail: 0
    };
    (window as any).__mock = s;
    (window as any).__calls = [];
    const log = (l: string) => s.lines.push(l);
    // A scripted start: six steps over 3 s, the first frame at 3.2 s (`?slowstart` stays on step 5).
    const bounds = [0, 10, 28, 50, 72, 92];
    const progress = (at: number) => {
      const t = at - s.startedAt;
      const slow = !!q.get('slowstart');
      const ready = !slow && t >= 3200 && s.game !== 'crashed';
      const percent = ready ? 100 : slow ? Math.min(81, (t / 3000) * 100) : Math.min(99, (t / 3000) * 100);
      const step = Math.max(1, bounds.filter((b) => percent >= b).length);
      return {
        percent, phase: ['apk', 'libraries', 'constructors', 'runtime', 'launch', 'frame'][step - 1], step,
        elapsed_secs: t / 1000, eta_secs: t > 3000 && percent >= 15 && !ready ? ((100 - percent) * t) / percent / 1000 : null,
        slow: slow && t > 3000, stalled: false,
        step_secs: bounds.map((_, i) => (i + 1 < step || ready ? 0.5 + i / 2 : null)), ready
      };
    };
    const snapshot = (since: number) => {
      const running = ['starting', 'running'].includes(s.game);
      if (s.game === 'starting' && !q.get('slowstart') && Date.now() - s.startedAt > 3200) s.game = 'running';
      if (running && s.crashAt && Date.now() > s.crashAt) {
        s.game = 'crashed';
        s.exit = 139;
        if (q.get('bootcrash')) s.bootFail = Date.now();
        log('20:14:07  ERROR    native crash in libgame.so (SIGSEGV)');
        s.crashAt = 0;
      }
      const lines = s.lines.slice(since);
      return {
        state: s.game,
        running_secs: ['starting', 'running'].includes(s.game) ? (s.game === 'running' && !s.startedAt ? 720 : Math.floor((Date.now() - s.startedAt) / 1000)) : null,
        last_session_secs: ['stopped', 'crashed'].includes(s.game) ? s.lastSecs : null,
        exit_code: s.game === 'crashed' ? s.exit : null,
        signal: null,
        lines,
        next: s.lines.length,
        progress: s.game === 'starting' || s.bootFail ? progress(s.bootFail || Date.now()) : s.game === 'running' ? progress(s.startedAt || 0) : null
      };
    };
    if (['crashed'].includes(s.game)) {
      s.lines = ['20:14:02  runtime  frame 18233 · 59 fps', '20:14:07  ERROR    native crash in libgame.so (SIGSEGV)'];
      s.exit = 139;
    }
    if (s.game === 'running') s.lines = ['[play] window 1600x900', '[play] first frame presented'];
    const report = (path: string, force: boolean) => {
      const name = path.split('/').pop() ?? path;
      const base = { path, size_bytes: 64_300_000, status: 'supported', version: '1.7.6', problem: null, detail: '' };
      if (name.includes('other')) return { ...base, status: 'error', version: null, problem: 'not_blockheads', detail: 'APK has no entry lib/armeabi-v7a/libApplication.so', size_bytes: 3_100_000 };
      if (name.includes('old'))
        return force
          ? { ...base, status: 'unknown', version: null }
          : { ...base, status: 'error', version: null, problem: 'unsupported', detail: 'unknown Blockheads build (libApplication.so sha256 4f2a…c91e)' };
      if (name.includes('damaged')) return { ...base, status: 'error', version: null, problem: 'damaged', detail: 'truncated APK', size_bytes: 12_400_000 };
      if (name.includes('denied')) return { ...base, status: 'error', version: null, problem: 'unreadable', detail: 'cannot read APK: Permission denied (os error 13)' };
      return base;
    };

    // ---- client mods (src-tauri/src/mods.rs) ----
    const mod = (o: any) => ({
      id: 'minimap', name: { en: 'Minimap' }, description: { en: 'A small map in the corner. Shows places you’ve seen and your friends on the same server.' },
      version: '0.9.1', authors: ['pixelpan'], license: 'MIT', homepage: 'https://github.com/pixelpan/minimap', sides: { server: false, client: true },
      runtime: 'lua', api: '^0.1', native: false, permissions: [], granted: [], missing: [], enabled: true, status: 'ready', reason: null, detail: null,
      dependencies: [], file: 'minimap-0.9.1.bhmod', sha256: 'ab'.repeat(32), size: 420_000, reviewed: true, update: null, update_asks_more: false,
      source: 'index', origin: '', installed_at: 1_759_449_600, native_allowed_at: null, ...o
    });
    s.mods = q.get('mods') === 'empty' ? [] : [
      mod({ sides: { server: true, client: true }, permissions: ['world:read', 'channels'], granted: ['world:read', 'channels'] }),
      mod({ id: 'hats', name: { en: 'Hats & Capes' }, version: '1.1.0', description: { en: 'Hats and capes for your blockheads.' } }),
      mod({ id: 'better-sounds', name: { en: 'Better Sounds' }, version: '1.3.0', update: '1.4.0', description: { en: 'Richer footsteps, rain and cave echoes.' } }),
      mod({ id: 'zoom-out', name: { en: 'Zoom Out' }, version: '2.0.0', enabled: false, status: 'off', permissions: ['world:read'], granted: ['world:read'] }),
      mod({ id: 'cozy-menus', name: { en: 'Cozy Menus' }, version: '0.3.0', status: 'permissions', permissions: ['world:read'], missing: ['world:read'], detail: 'world:read' }),
      mod({ id: 'day-sky', name: { en: 'Day & Night Sky' }, version: '1.0.0', status: 'dependency', detail: 'sky-core ^1', authors: ['lumen'],
        dependencies: [{ id: 'sky-core', req: '^1', installed: null, ok: false, soft: false, available: '1.2.0' }] }),
      mod({ id: 'pixel-shaders', name: { en: 'Pixel Shaders' }, version: '0.7.2', native: true, permissions: ['native'], granted: ['native'], reviewed: false,
        source: 'file', authors: ['glowbug'], license: '', homepage: null, native_allowed_at: 1_760_227_200, description: { en: 'Soft lighting and glow effects. Uses native code for speed.' } })
    ];
    const overview = () => {
      if (q.get('mods') === 'error') throw { code: 'io', detail: 'Permission denied (os error 13)', file: '/home/mira/.local/share/blockheads-pc/mods' };
      const att = s.mods.filter((m: any) => ['failed', 'blocked', 'dependency', 'permissions', 'budget'].includes(m.status)).length;
      return { dir: '/home/mira/.local/share/blockheads-pc/mods', size: 3_400_000, mods: s.mods, problems: [], error: null, attention: att, updates: 1, loader_ready: true };
    };
    const bmod = (o: any) => ({ id: 'sky-core', name: 'Sky Core', description: 'Shared sky and weather code other mods use.', owners: ['lumen'], repo: 'https://github.com/lumen/sky-core',
      tags: ['library'], version: '1.2.0', compatible: true, compatible_version: '1.2.0', sides: ['client'], permissions: [], native: false, depends: {}, size: 210_000,
      published: '2026-10-01', versions: [['1.2.0', '2026-10-01'], ['1.1.0', '2026-09-02']], installed: null, ...o });
    const browse = () => ({ generated: '2026-10-21T12:00:00Z', offline: false, error: null, tags: ['fun', 'graphics', 'interface', 'library', 'sound'], mods: [
      bmod({}),
      bmod({ id: 'cozy-menus', name: 'Cozy Menus', owners: ['pixelpan'], tags: ['interface'], description: 'Softer, rounder menus with bigger text.', installed: '0.3.0', version: '0.3.0' }),
      bmod({ id: 'pets', name: 'Pet Companions', owners: ['pixelpan'], tags: ['fun'], description: 'Tame a dodo; it follows you, with its own hat.', sides: ['server', 'client'], permissions: ['world:read', 'channels'] }),
      bmod({ id: 'turbo', name: 'Turbo Lighting', tags: ['graphics'], description: 'Smooth lighting and coloured torches. Native code for speed.', native: true, permissions: ['native'] }),
      bmod({ id: 'keys', name: 'Key Hints', owners: ['echo'], tags: ['interface'], description: 'Shows keyboard and controller hints on screen.', version: '2.0.0', compatible: false, compatible_version: '1.6.0' }),
      bmod({ id: 'better-sounds', name: 'Better Sounds', owners: ['echo'], tags: ['sound'], description: 'Richer footsteps, rain and cave echoes.', installed: '1.3.0', version: '1.4.0' }),
      bmod({ id: 'land-claims', name: 'Land Claims', owners: ['tobyblock'], tags: [], sides: ['server'], description: 'Claim land on a server.' })
    ] });
    const plan = (id: string) => {
      const item = (o: any) => ({ id: 'day-sky', name: 'Day & Night Sky', version: '1.0.0', size: 640_000, sides: ['client'], permissions: ['world:read'], native: false,
        needed_by: null, replaces: null, reviewed: true, authors: ['lumen'], ...o });
      if (id === 'turbo') return { items: [item({ id: 'turbo', name: 'Turbo Lighting', version: '2.0.1', native: true, permissions: ['native'], size: 2_400_000 })], satisfied: [], missing: [], total_bytes: 2_400_000, token: null };
      if (id === 'keys') throw { code: 'newerLoader', file: 'Key Hints 2.0.0', expected: '^1.4', got: '0.1.0', detail: '1.6.0' };
      return { items: [item({}), item({ id: 'sky-core', name: 'Sky Core', version: '1.2.0', size: 210_000, permissions: [], needed_by: 'day-sky' })], satisfied: [], missing: [], total_bytes: 850_000, token: null };
    };
    s.servers = { total: 5_100_000, servers: [
      { key: 'mira.example.net_15151', name: 'Sunny Isle', address: 'mira.example.net:15151', last_played: Math.floor(Date.now() / 1000) - 3600, size: 3_900_000, mods: [
        { id: 'minimap', name: 'Minimap', version: '0.9.1', sha256: 'a1', size: 420_000, native: false, shared_with: ['Dodo Land'] },
        { id: 'pets', name: 'Pet Companions', version: '1.2.0', sha256: 'b2', size: 1_100_000, native: false, shared_with: [] },
        { id: 'turbo', name: 'Turbo Lighting', version: '2.0.1', sha256: 'c3', size: 2_400_000, native: true, shared_with: [] }] },
      { key: '192.168.1.20_15151', name: 'Dodo Land', address: '192.168.1.20:15151', last_played: Math.floor(Date.now() / 1000) - 14 * 86400, size: 1_200_000, mods: [
        { id: 'minimap', name: 'Minimap', version: '0.9.1', sha256: 'a1', size: 420_000, native: false, shared_with: ['Sunny Isle'] },
        { id: 'race', name: 'Race Track', version: '0.5.0', sha256: 'd4', size: 780_000, native: false, shared_with: [] }] }
    ] };
    const modHandlers: Record<string, (a: any) => any> = {
      mods_overview: overview,
      mods_index: async () => {
        if (q.get('modsindex') === 'slow') await new Promise(() => {});
        if (q.get('modsindex') === 'offline') throw { code: 'offline', detail: 'dns error: failed to lookup address' };
        return browse();
      },
      mods_plan: (a) => plan(a.id),
      mods_inspect_file: () => ({ items: [{ id: 'pixel-shaders', name: 'Pixel Shaders', version: '0.7.2', size: 1_300_000, sides: ['client'], permissions: ['native'], native: true,
        needed_by: null, replaces: null, reviewed: false, authors: ['glowbug'] }], satisfied: [], missing: [], total_bytes: 1_300_000, token: 'tok' }),
      mods_inspect_link: (a) => {
        if (!a.url.startsWith('https://')) throw { code: 'insecure', detail: a.url };
        throw { code: 'checksum', file: 'pixel-shaders-0.7.2.bhmod', expected: '9f2c…a71e', got: '41b0…0c3d' };
      },
      mods_install: async () => { await new Promise((r) => setTimeout(r, 300)); return ['day-sky']; },
      mods_set_enabled: (a) => { s.mods = s.mods.map((m: any) => (m.id === a.id ? { ...m, enabled: a.on, status: a.on ? 'ready' : 'off' } : m)); },
      mods_set_grant: () => undefined,
      mods_remove: (a) => { s.mods = s.mods.filter((m: any) => m.id !== a.id); },
      mods_auto_update: () => ({ updated: [], failed: [] }),
      mods_servers: () => s.servers,
      mods_clear_server: (a) => ((s.servers = { ...s.servers, servers: s.servers.servers.filter((x: any) => x.key !== a.key) }), s.servers),
      mods_clear_servers: () => ((s.servers = { total: 0, servers: [] }), s.servers),
      mods_dev_inspect: (a) => ({ path: a.path, id: 'my-mod', version: '0.1.0-dev', name: 'My mod', permissions: [], native: false,
        errors: ['client/ui.luau: entry point is missing'], warnings: ['LICENSE: missing (needed for the public list)'] }),
      mods_dev_reload: () => { if (s.game !== 'running' && s.game !== 'starting') throw { code: 'notRunning' }; },
      mods_dev_fingerprint: () => 'fp',
      mods_dev_join: () => handlers.game_start({})
    };
    const update = { version: '0.2.0', size_mb: 43, notes: ['Smoother camera panning', 'Fixes a crash when importing saves', 'Turkish translation'] };
    const handlers: Record<string, (a: any) => any> = {
      app_info: () => ({ version: '0.1.0', os, arch: 'x86_64', config_dir: '/home/mira/.config/blockheads-pc', updates_enabled: !!q.get('updates'), native_controls: os === 'macos' && !!q.get('native') }),
      boot: async () => {
        if (q.get('splash') === 'hang') await new Promise(() => {});
        if (q.get('splash') === 'error') throw 'i/o error: Permission denied (os error 13)';
        return { config: s.config, defaults, prefs: s.prefs, prefs_saved: !q.get('fresh') };
      },
      splash_set: () => undefined,
      splash_get: () => ({
        boot: q.get('splash') === 'error' ? 'error' : q.get('splash') === 'slow' ? 'slow' : 'loading',
        error: q.get('splash') === 'error' ? 'i/o error: Permission denied (os error 13)' : '',
        first_run: !!q.get('first'),
        version: '0.1.0',
        config_dir: '/home/mira/.config/blockheads-pc',
        lang: q.get('lang') ?? 'en'
      }),
      splash_retry: () => undefined,
      app_quit: () => undefined,
      main_ready: () => undefined,
      save_config: (a) => ((s.config = a.config), s.config),
      save_prefs: (a) => ((s.prefs = a.prefs), s.prefs),
      finish_setup: (a) => ((s.config = { ...a.config, apk: '/home/mira/.local/share/blockheads-pc/game/TheBlockheads.apk' }), (s.prefs = a.prefs), s.config),
      check_apk: (a) => report(a.path, a.force),
      use_apk: (a) => ((s.config = { ...s.config, apk: a.path, force: a.force }), s.config),
      normalize_binding: (a) => {
        if (/^F1[3-9]$/.test(a.text)) throw `invalid binding: ${a.text}`;
        return a.text.replace(/^Escape$/i, 'Esc');
      },
      display_info: () => (q.get('nodisplay') ? null : { width: 2560, height: 1440, scale: 1.25 }),
      window_resize: () => undefined,
      import_saves: (a) =>
        a.source.includes('empty')
          ? { written: 0, skipped_existing: 0, ignored: 0, items: [] }
          : { written: 14, skipped_existing: 1, ignored: 2, items: [{ name: 'saves/3f1e9c0a', files: 6, bytes: 48_000_000 }, { name: 'saves/b52d7714', files: 5, bytes: 9_100_000 }, { name: 'Documents/profile.dat', files: 3, bytes: 12_000 }] },
      data_size: () => 222_000_000,
      move_data_dir: (a) => ((s.config = { ...s.config, data_dir: a.to }), s.config),
      open_path: () => undefined,
      game_start: () => {
        if (q.get('failstart')) throw 'cannot start the game: /opt/bhpc-run: No such file or directory';
        s.lines = [];
        s.game = 'starting';
        s.startedAt = Date.now();
        s.bootFail = 0;
        log('[play] window 1600x900');
        log('[play] first frame presented');
        if (q.get('crash')) s.crashAt = Date.now() + 5500;
        if (q.get('bootcrash')) s.crashAt = Date.now() + 1800;
        return { embedded: false };
      },
      embed_layout: () => undefined,
      embed_reveal: () => undefined,
      embed_focus: () => undefined,
      embed_fullscreen: () => undefined,
      game_stop: () => {
        s.game = 'stopped';
        s.lastSecs = 125;
      },
      game_poll: (a) => snapshot(a.since),
      save_log: () => undefined,
      list_mics: () => ['USB Mic'],
      update_check: () => {
        if (q.get('update') === 'error') throw 'network unreachable';
        return q.get('update') === 'latest' ? null : update;
      },
      update_install: async () => {
        s.installStart = Date.now();
        await new Promise((r) => setTimeout(r, 4000));
      },
      update_progress: () => {
        const t = (Date.now() - s.installStart) / 4000;
        return { stage: t < 0.7 ? 'downloading' : 'installing', percent: Math.min(100, Math.round(t * 100)), done_mb: Math.round(t * 43), total_mb: 43, mb_per_s: 1.4, eta_s: Math.max(0, Math.round(30 * (1 - t))) };
      },
      'plugin:dialog|open': (a) => (a.options?.directory ? (s.pick.endsWith('.apk') || s.pick.endsWith('.zip') ? '/home/mira/Desktop/blockheads-saves' : s.pick) : s.pick),
      'plugin:dialog|save': () => '/home/mira/blockheads-pc.log',
      'plugin:opener|open_url': () => undefined,
      'plugin:window|is_maximized': () => !!q.get('maximized'),
      'plugin:window|minimize': () => undefined,
      'plugin:window|toggle_maximize': () => undefined,
      'plugin:window|close': () => undefined,
      'plugin:window|is_maximized': () => false,
      'plugin:window|is_fullscreen': () => false,
      'plugin:window|minimize': () => undefined,
      'plugin:window|toggle_maximize': () => undefined,
      'plugin:window|close': () => undefined,
      'plugin:window|destroy': () => undefined,
      'plugin:event|listen': () => 1,
      'plugin:event|unlisten': () => undefined,
      ...modHandlers
    };
    (window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    (window as any).__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main', windowLabel: 'main' } },
      transformCallback: () => 0,
      invoke: async (cmd: string, a: any) => {
        (window as any).__calls.push([cmd, a]);
        const h = handlers[cmd];
        if (!h) throw new Error('unmocked ' + cmd);
        return h(a ?? {});
      }
    };
  });
}
