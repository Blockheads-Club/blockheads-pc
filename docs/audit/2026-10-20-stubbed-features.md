# Audit: features that silently do nothing (2026-10-20)

Scope: find every game feature that is a silent no-op on PC because a Java class or method is only a lenient stub
in `bhpc-jni`, because an HLE call fails (bionic/vfs/vfd), or because the host never shows or answers something.
This is an audit only: no code was changed. Three items are being fixed in parallel and are left out here: text
input fields, the screen aspect ratio and menu music (done).

Machine-readable list: `~/.cache/bhpc-audit/items.json` (`id, title, class, evidence, fix_location, size,
depends_on`). Key logs: `~/.cache/bhpc-audit/keylogs/`.

Classes:
- **A**: a user-visible feature that is broken on PC (must fix).
- **B**: should be mapped to a PC equivalent.
- **C**: harmless or meaningless on PC (checked that it fails gracefully).
- **D**: unknown, needs investigation.

Counts: **A 7, B 9, C 9, D 6**.

## Method

- **Static.** Lenient paths in `crates/bhpc-jni` (`vm.rs` find_class/method_id/jfield_id invent members; `invoke.rs`
  returns the type's zero value). The jlib class inventory. The HLE failure tables in `bhpc-bionic/src/sys.rs`,
  `bhpc-run/src/vfd*`. Strings of the game's `libApplication.so`/`libCFNetwork.so`/`libBridgeKit.so` (read from the
  APK through a pipe; nothing was extracted).
- **Dynamic.** 49 headless runs of the release `bhpc-run` (dev @ 515ec88 plus the other agent's working tree as built
  at 16:48). Each run used Xvfb on its own display number, `dbus-run-session` with a private bus, a temp HOME and a
  scratch data dir under `~/.cache/bhpc-audit/`. Every run had `--jni-report`, and most also had
  `--trace-jni --trace-jni-calls --trace-objc` and `BHPC_TRACE_NET=1`. Every run starts from a copy of one template
  data dir, so the stub/selector set a flow adds is the diff against baseline runs (`analyze.sh`). The runner,
  scripts and analyzer are in `~/.cache/bhpc-audit/{run.sh,analyze.sh,scripts/}` and can be run again.
- `--jni-report` lists **532 looked-up-but-stubbed methods in 84 invented classes**. Only 48 of them were called in
  any flow (`keylogs/called-stubs-all-runs.tsv`). The rest are the latent surface (Intent extras, Calendar,
  SocialShareManager, WebView, ...).

### Flows driven

| Area | Flows (run names) | Result |
|---|---|---|
| Main menu | boot, settings, moregames, crystals, join, create, options | menus OK; More Games does nothing (B1) |
| Settings | set-dbltime, set-classic, set-control, set-audio, set-privacy, persist-a/b/c | screens OK; Restore auto-confirmed (A1); settings lost (A2/A3) |
| Store | crystals, buy, freeoffers | prices stay `...` (B7); Free Offers opens nothing (B1/C4) |
| World list | options, delete, migrate, create, create-go, create-help, create-online | game-drawn popups OK; LAN host button missing (A5) |
| Multiplayer | join-random, join-rand-go, join-adv, join-ip, join-search, join-pic | cloud offline (C5); no LAN join (A5/A6); pic picker no-op (B3) |
| In world | world-enter, world-warp, world-customize, world-warpin, mk-tmpl2, w-bh, portal-tabs, inv | warp-in, customize, portal crafting and inventory work |
| Pause menu | p-links, l-app, l-twitter, l-facebook, l-forums, p-help, p-achieve, p-crystals, p-options | links (B1), Help/Credits (A4) and Achievements (B8) do nothing |
| Lifecycle/keys | life-menu, life-world, esc-menu, esc-world | Escape on the menu quits at once (A1); background works |

Not reached: chat, signs, camera photos, invite/share, hosting (they need crafting, an online host or the dead
cloud) and the text-field flows (excluded). See D6.

## A: user-visible breakage (must fix)

| id | title | size | fix location | depends on |
|---|---|---|---|---|
| A1 | Button-only system alerts are answered automatically and never shown | M | `bhpc-run/src/play.rs` ui_loop/`answer`; `bhpc-jni/src/render/dialog.rs` | the text-input dialog work |
| A2 | Settings and consent are not restored on the next start | M | CFPreferences load path: `bhpc-bionic` stat/vfs, `.fscache` lookups | |
| A3 | Closing the window skips the app lifecycle (no background or terminate) | S | `bhpc-run/src/play.rs` close path | A2 |
| A4 | Help/Credits (NoodleWebView) does nothing | M | `bhpc-jni/src/jlib/apportable.rs` + `render.rs` or the browser | B1 |
| A5 | LAN host/join buttons missing: the Reachability callbacks never fire | S | jlib: a real `com/apportable/net/Reachability` | |
| A6 | LAN discovery backend: mDNS needs AF_UNIX sockets and interface ioctls | M | `bhpc-run/src/vfd/net.rs`, `vfd.rs` ioctl | A5 |
| A7 | Windows builds have no guest sockets (no multiplayer) | L | `bhpc-run/src/vfd/net_stub.rs` -> Winsock | |

**A1: system alerts are auto-answered.** `ui_loop` takes every `AlertView` and, unless it has a text field, calls
`answer(c, &alert, None)`, which presses the first button. Nothing is drawn: `render/dialog.rs` only handles
alerts with an input line. Evidence:
- `[play] alert "Enable multiplayer and free crystals?" ... -> pressing 1` ("Allow") on **every** boot. Privacy
  consent and ads are accepted without the user ever seeing the question.
- esc-menu: Escape on the main menu -> `"Exit?": "Are you sure you want to exit The Blockheads?"; buttons [("Yes", 1)]
  cancel Some(("No", 0)) -> pressing 1`, then `[play] ended: alert: guest stopped`. The game quits without asking.
- set-dbltime: "Restore Double-Time" -> "Restore" is pressed automatically.

Fix: show these alerts in the existing host dialog (title, message, buttons; Escape = cancel). Coordinate with the
agent that is working on `render/dialog.rs`.

**A2: preferences are written but not applied at the next start.**
- persist-a toggled Classic look ON and then minimised. `Library/Preferences/blockheads.plist` was written with
  `{gdprStatus: 2, hdTexturesDisabled: True, GPGSShouldSignIn: True, ...}` (copy in `keylogs/`).
- persist-b and persist-c started from that data dir. `--trace-fs` shows the plist opened 3 times, each followed by an
  odd `files/.0x40a952b0.fscache` lookup.
- `--trace-fn` shows the file read (`CFURLCreateDataAndPropertiesFromResource`) and parsed
  (`CFPropertyListCreateWithData`) 3 times, and `CFPreferencesCopyAppValue("gdprStatus")` called.
- Still, Options shows Classic look OFF and the consent alert appears again. Some step of the CFPreferences domain
  load rejects the file: perhaps a stat/mtime mismatch, the `.fscache` handling, or the parse.

Start the investigation in `bhpc-bionic` stat/fstat results for that path and in what CF does with `.fscache`.

**A3: no lifecycle on exit.** A window close (`flags.done`) or a script quit ends `ui_loop` with no
`WillResignActive`/`DidEnterBackground`/`WillTerminate`.
- In every run that quit without `minimize`, `Library/Preferences/` stayed empty.
- With `minimize`, the game runs `applicationDidEnterBackground:` -> `persistNotifications` -> `synchronize` and writes
  the plist.
- In a world, progress since the last autosave (`save delay:10`) is also at risk.

Fix: on close, send the background/terminate natives, wait for the save, then shut down.

**A4: Help/Credits.** p-help: `instructionsButton:` -> `showBlockheadsWebView:editable:id:` ->
`STUB static com/noodlecake/noodlewebview/NoodleWebView.showBlockheadsWebView(Ljava/lang/String;ZJ)V x1`, and no view
appears. The same class shows the custom-rules help (`customRulesHTMLTextForRules`) and the *editable* server
welcome message (`BlockheadsWebView.nativeSetWelcomeMessage`).

Smallest fix: write the HTML to a temp file and open it in the browser (needs B1). Better: an in-game text panel
(strip the HTML) in `render.rs`, with an edit mode for the welcome message.

**A5: LAN UI hidden.**
- `com/apportable/net/Reachability.<init>`/`start()` are stubs. Its natives `didStart`, `stateChanged(III)` and
  `stateChangeFinished` are never called (x0).
- The game logs `Reachability Flag Status: WR -------`: W = `IsWWAN`, so the game believes it is on cellular.
- The binary has `JoinWorldUI.advancedLocalNetworkButton`, `LoadWorldUI.hostMultiLANButton`,
  `"HOST LOCAL NETWORK..."` and `localWiFiStatusForFlags:`. Yet Join -> Advanced shows only "Online IP/Port..." and
  "Search...", and world Options shows only Back/Delete/Name/Migrate.

Hypothesis (strong, unverified): the LAN buttons need WiFi reachability. Fix: implement Reachability.start to call
`didStart` and then `stateChanged(<wifi>, <connected>, ...)` + `stateChangeFinished`. Also
`WifiManager.isWifiEnabled -> true` and a no-op MulticastLock.

**A6: mDNS backend.** libCFNetwork embeds the mDNSResponder posix daemon (`mdns_posix_daemon_main`,
`DNSServiceBrowse`) and imports `ioctl`.
- Every run logs `[net] socket([1, 1, 0, 0]) = -1 errno 97` (AF_UNIX stream, EAFNOSUPPORT) x4. dns_sd clients talk
  to the daemon over a unix socket.
- vfd's `ioctl` answers only FIONREAD/FIONBIO, so mDNSPosix's interface enumeration (`SIOCGIFCONF`, `SIOCGIFFLAGS`,
  `SIOCGIFNETMASK`, ...) gets EINVAL.

So LAN discovery will probably still fail after A5. Needed: AF_UNIX stream sockets in `vfd/net.rs` and the interface
ioctls in `vfd.rs`. Multicast setsockopt already passes through to the host.

**A7: Windows.** `vfd/net_stub.rs` leaves bionic's failing socket stubs in place on non-unix hosts, so there is no
online or LAN play on Windows. Found statically, not run.

## B: map to a PC equivalent

| id | title | size | fix location | depends on |
|---|---|---|---|---|
| B1 | URL opening is a no-op (More Games, Links x4, Free Offers, support) | S | `jlib/android.rs` Uri/Intent/PackageManager/startActivity + host opener | |
| B2 | Email: `canSendEmail=false`, `sendEmail` no-op | S | `jlib/apportable.rs` -> mailto via B1 | B1 |
| B3 | "Your pic" picker: `startActivityForResult` stub | M | jlib Intent + native file dialog + `nativeOnActivityResult` | B1 |
| B4 | Game-side clipboard (ClipboardManager/ClipData) stubbed | S | `jlib/android.rs` -> SDL clipboard | |
| B5 | Share/invite (SocialShareManager, invite mail) stubbed | M | `jlib/apportable.rs` -> clipboard/mailto | B1, B4 |
| B6 | Camera photos -> `UIImageWriteToSavedPhotosAlbum` -> MediaScanner stub | S | verify; map sdcard/Pictures -> ~/Pictures | |
| B7 | Store: prices stay `...`, buy no-op, Restore "verification failed" | S | ApportableIAP firing "billing unavailable" | |
| B8 | View Achievements does nothing (Play Games stub) | M | local achievements panel or a message | |
| B9 | Incoming `join.php?ip=...` links never reach the game | M | URL handler/CLI -> `nativeHandleUri` | B1 |

**B1** (the most visible). Every external link goes through `UIApplication openURL:`. On Android that becomes:
1. `Uri.parse` (STUB, returns NULL);
2. `new Intent(ACTION_VIEW, uri)` (STUB);
3. `PackageManager.resolveActivity` / `queryIntentActivities` (STUB, return NULL);
4. so it stops there, silently.

Seen in moregames, l-app, l-twitter, l-facebook, l-forums and freeoffers (`canOpenURL:`). Example selector sequence:
`moreGamesButton: showMoreGames openURL: _isAppURL: ... parse: initWithAction:uri: resolveActivity:flags:
setPackage: queryIntentActivities:flags:`.

URLs in the binary:
- `https://news.noodlecake.net/more_games/?bundle_id=%@&platform=google`
- `https://play.google.com/store/apps/details?id=com.noodlecake.blockheads`
- `https://twitter.com/MJTheBlockheads`
- `http://m.facebook.com/296991433712167`
- `https://forums.theblockheads.net`
- `mailto:support+android@noodlecake.com?...`
- `http://portal.theblockheads.net/login?sId=%@`

Fix:
- Give the fake `Uri` its string and the `Intent` its action/data/extras.
- Return a ResolveInfo/one-element list for VIEW http(s)/mailto/market.
- Route `Context.startActivity`/`Activity.startActivity` to a new host hook that opens the URL with the system
  opener. Honour `BROWSER`; headless tests set `BROWSER=true`.
- Log each opened URL. Map `market://` to the Play web page.

**B3.** join-pic: `picButton:` -> `startImagePickerWithDelegate:forRect:cropSize:` -> `startActivity:forResult:` ->
`STUB android/app/Activity.startActivityForResult(Landroid/content/Intent;I)V x1`. `nativeOnActivityResult` is never
called. Needs a native file dialog, then `nativeOnActivityResult(requestCode, RESULT_OK, intent with file Uri)` and a
`ContentResolver.openInputStream` for that Uri.

**B7.** The store shows `...` on every pack: `querySkus` is a stub and `setupFinished` is never fired. Clicking a pack
does nothing. Restore (once A1 shows the alert) ends with "Purchase Verification Failed: Your purchase could not be
verified with Google's servers." Firing the IAP setup callback with "unsupported" should make the game show its own
"unavailable" state. Otherwise hide the store via `gameui`.

**B8.** p-achieve: `achievementsButtonTapped` runs and nothing visible follows. `PlusGamesManager` (35 methods looked
up) is a stub with `isLoggedIn=false`. Achievement ids are mapped to Play Games at boot. Options: a local
achievements list (record the unlock calls) or a host message such as "not available on PC".

## C: harmless on PC (graceful failure confirmed)

| id | item | evidence |
|---|---|---|
| C1 | Accelerometer (`cm/MotionManager`) | start/stop stubs in every world run; no tilt control; no errors |
| C2 | Vibration (`Vibrator.hasVibrator/vibrate`) | x0; `isVibrationSupported` reads false |
| C3 | Local notifications (`Notifications.*`, `LocalNotification`) | NULL/void, called at boot and on background; fine |
| C4 | Ads/analytics (Heyzap, Flurry, NoodleNews, AdvertisingId) | all stubs; `IncentivizedAd.isAvailable` returns a NULL Boolean without a crash; Free Offers shows no ad (could hide it) |
| C5 | Official cloud (`cloud.theblockheads.net`) offline | the host cannot resolve it either; the game shows its own "SEARCH FAILED"; IP join works (M7) |
| C6 | Locale fixed `en_US` | the APK ships only `en.lproj` |
| C7 | WakeLock, TelephonyManager, `_setSystemUiVisibility`, orientation | no-ops |
| C8 | Cosmetic UI stubs (`setButtonType`, `setLineBreakMode`, `setNeedsDisplay`, `invalidateObject`, `SoftReference`) | popups render correctly in screenshots |
| C9 | Double-Time restore failure | the game's own message (after A1) |

## D: needs investigation

| id | item | next step |
|---|---|---|
| D1 | Who makes the 4 AF_UNIX `socket()` calls per run | `--trace-fn socket` + backtrace; likely dns_sd (A6) |
| D2 | `Label.setLineBreakMode` ignored (x88) | compare truncation with Android |
| D3 | Trade price server (`http://priceserver.theblockheads.net/...`) | check in a world with a trade portal; Apache HTTP classes are stubs, CFNetwork works |
| D4 | `DefineClass` always fails (DexMaker proxies) | no failing flow seen |
| D5 | `imports still bound to traps: dladdr inet_nsap_ntoa inet_nsap_addr AndroidBitmap_*` (never hit) | check whether the AndroidBitmap ones are a stale report |
| D6 | Flows not reached: chat, signs, photos, invite, hosting, text-field flows | run again after A1/A5 |

## Dependencies

- B1 (URL/Intent model) comes first for B2, B3, B5, B9 and the cheap A4 path.
- A5 (Reachability) comes before A6 (mDNS backend). Both are needed for working LAN play. A7 is Windows only.
- A2 and A3 together decide whether settings survive a restart. A1 shares the dialog code with the text-input work
  that is in progress.

## Static inventory (summary)

- **Lenient paths in `bhpc-jni`.**
  - `vm.rs` `find_class` invents unknown classes (`jni: stub class`).
  - `method_id` and `jfield_id` invent members (`jni: stub method|field`).
  - `invoke.rs` first tries a `Java_*` export, then returns the zero value (false/0/NULL).
  - Unset fields read as zero.
  - Unknown `getSystemService` names give NULL. Known names (vibrator, clipboard, notification, sensor, wifi, power,
    phone, location, alarm) give a bare object whose methods are stubs.
  - `DefineClass` always throws.
- **Reports.** `--jni-report FILE` has `class-stub`, `host|native|STUB ... xN` and `field` lines. At exit,
  `java stub called: ...` lists only stubs with calls > 0.
- **Implemented Java.**
  - android.os (Build, Process, SystemClock, Environment, Bundle).
  - Context/Activity paths and services; in-memory SharedPreferences; Resources/AssetManager/Display.
  - Log (2-arg only); PackageManager.hasSystemFeature/getPackageInfo; AudioManager constants; ConnectivityManager
    (always WiFi); AudioTrack; Apportable AudioPlayer.
  - VerdeActivity: `canSendEmail` and `hasActivityForAction` are constant false, `sendEmail` is a no-op.
  - MainThread/RunLoop/Lifecycle; com.apportable.ui views (View, Window, AlertView, TextField/TextView, Label, Button,
    ImageView; other widgets inert).
  - Bitmap/Typeface; File/streams; java.lang basics, collections; Locale/TimeZone/UUID; Looper/Handler.
- **Not implemented (stubs).**
  - HTTP (Apache), IAP, Play Games, NoodleNews/NoodleWebView/WebView, ads/analytics, Notifications, Reachability,
    MotionManager.
  - Toast, Intent/Uri/ClipData, AlertDialog/Dialog, MediaPlayer/MediaScanner, Calendar, DecimalFormatSymbols,
    android.view.*/Canvas/Paint, EGL10, SocialShareManager.
- **HLE failures.**
  - `bionic/sys.rs`: fixed-errno table: exec* ENOENT, fork EAGAIN, chmod/utime ENOENT, symlink EACCES.
  - getaddrinfo is overridden by vfd/net on unix.
  - Unresolved imports are bound to traps that **stop** the game when called (listed at exit).
  - `vfd.rs` ioctl only FIONREAD/FIONBIO.
  - `vfd/net.rs`: AF_INET sockets only (AF_UNIX -> EAFNOSUPPORT); `net_stub.rs` has no sockets on Windows.
  - vfs: no ENOSYS paths.
