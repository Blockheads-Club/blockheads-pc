import type { Page } from '@playwright/test';

/**
 * Replaces the Tauri IPC with an in-page fake launcher backend so the built web UI can be driven in a browser.
 * Scenarios come from the URL query: ?first=1 (no saved config), ?os=macos|windows|linux, ?game=ready|running|
 * stopped|crashed|missing, ?updates=1 (the update feed exists), ?update=available, ?splash=hang|error (main window boot) and, on /splash, ?splash=slow|error&first=1 (state of the splash window).
 */
export async function mockTauri(page: Page) {
  await page.addInitScript(() => {
    const q = new URLSearchParams(location.search);
    const os = q.get('os') ?? 'linux';
    const defaults = {
      apk: '',
      data_dir: '/home/mira/.local/share/blockheads-pc',
      force: false,
      window: { width: 1280, height: 720, fullscreen: false, vsync: true, ui_scale: 1, show_fps: false, fps_position: 'top-left', fps_limit: 60, game_speed: 1 },
      keys: { touch: 'Left', pan: 'Right', zoom: 'Wheel', pinch: 'Ctrl+Left', back: 'Esc', fullscreen: 'F11' }
    };
    const saved = {
      ...defaults,
      apk: '/home/mira/.local/share/blockheads-pc/game/TheBlockheads.apk',
      window: { width: 1600, height: 900, fullscreen: false, vsync: true, ui_scale: 1.25, show_fps: false, fps_position: 'top-left', fps_limit: 60, game_speed: 1 }
    };
    const s: any = {
      config: q.get('first') ? null : structuredClone(saved),
      prefs: { language: 'en', theme: 'system', auto_update_check: true, keep_private_copy: true, game_sound: true, mute_in_background: false },
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
    const update = { version: '0.2.0', size_mb: 43, notes: ['Smoother camera panning', 'Fixes a crash when importing saves', 'Turkish translation'] };
    const handlers: Record<string, (a: any) => any> = {
      app_info: () => ({ version: '0.1.0', os, arch: 'x86_64', config_dir: '/home/mira/.config/blockheads-pc', updates_enabled: !!q.get('updates'), native_controls: os === 'macos' && !!q.get('native') }),
      boot: async () => {
        if (q.get('splash') === 'hang') await new Promise(() => {});
        if (q.get('splash') === 'error') throw 'i/o error: Permission denied (os error 13)';
        return { config: s.config, defaults, prefs: s.prefs };
      },
      splash_set: () => undefined,
      splash_get: () => ({
        boot: q.get('splash') === 'error' ? 'error' : q.get('splash') === 'slow' ? 'slow' : 'loading',
        error: q.get('splash') === 'error' ? 'i/o error: Permission denied (os error 13)' : '',
        first_run: !!q.get('first'),
        version: '0.1.0',
        config_dir: '/home/mira/.config/blockheads-pc',
        lang: 'en'
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
      embed_fullscreen: () => undefined,
      game_stop: () => {
        s.game = 'stopped';
        s.lastSecs = 125;
      },
      game_poll: (a) => snapshot(a.since),
      save_log: () => undefined,
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
      'plugin:event|unlisten': () => undefined
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
