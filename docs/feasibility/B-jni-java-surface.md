# Analysis B: Java/JNI surface the native code relies on

Scope: Blockheads 1.7.4 (APKPure) armeabi-v7a. All numbers come from static analysis of the user-supplied APK (ELF readelf/objdump, dexdump, a small ObjC-metadata parser). No game code or assets are reproduced here.

## 0. Headline findings

1. **Almost nothing talks to Java through hand-written JNI.** `libApplication.so` contains no `FindClass`/`GetMethodID` strings. Native-to-Java calls go through **libBridgeKit.so**, an Apportable ObjC-to-Java reflection bridge. It defines 246 ObjC wrapper classes (`Android*` 154, `Java*` 54, `Verde*` 38), 928 wrapper methods, and registers ~498 Java class names with ~1,275 member/property names. Java method names and signatures are built at runtime from ObjC selectors and type encodings (`JavaMethodSignature`). That is why only 13 signature strings exist in the binary.
2. **The Java side is a thin shell.** Of 11,572 classes in the three dex files, only 417 (`com.apportable.*` + `com.noodlecake.*`) matter. The other ~11,100 are ad/analytics/Play-services SDKs (Heyzap, Fyber, AppLovin, Unity Ads, Chartboost, Vungle, Flurry, Moat, GMS, Volley, support-v4) that can be dropped.
3. **Several heavy things need no Java at all:**
   - Asset and file I/O: libv bundles zzip and wraps `open/fopen/stat/opendir/...` (`__wrap_*`), serving `assets/` and `com.apportable.system` from the APK/OBB zip. Paths come from env vars (`PACKAGE_CODE_PATH`, `PACKAGE_RESOURCE_PATH`, `OBBDIR`, `HOME`, `TMPDIR`, ...) set by `LibraryLoader.setenv`.
   - Multiplayer: ENet is statically linked into libApplication (159 `enet_*` strings) and uses libc sockets (`__wrap_socket/sendto/recvfrom`, `getaddrinfo`, `poll/select/epoll`).
   - Image decode (Skia + libpng in libCoreGraphics/GraphicsServices), text raster (FreeType + Pango), zlib/XML/ICU are all native.
4. **Java is genuinely needed for:** the app lifecycle and threading model (MainThread/Looper/MessageQueue/RunLoop), EGL/surface ownership, touch/key delivery, HTTP (Apache `HttpClient` via `HttpMessageShim`/`VerdeResponseHandler`), audio output/music (AudioTrack or OpenSL, `MediaPlayer` for the 9 mp4 music files), UIKit widgets (`com.apportable.ui.*`: alerts, text fields, web views), and the platform stubs (IAP, ads, Play Games, NoodleNews, notifications, reachability).
5. **Rough size of the fake-Java layer:** boot-to-menu is about 28 Java-to-native entry points and about 45 Java classes / 130-180 member IDs, of which 40-60 need real behaviour. Full gameplay is about 55 entry points (plus ~60 more for features that are stubbed) and about 90 classes / 350-450 member IDs, of which 120-160 need real behaviour. Basis and uncertainty are in section 5.

## 1. Artifacts and method

| Item | Value |
|---|---|
| APK | `The Blockheads_1.7.4_APKPure.apk` (1.7.5/1.7.6 not diffed here) |
| Native libs | 27 `.so` in `lib/armeabi-v7a/` (libApplication 17.2 MB, libicu 4.4 MB, libpango 3.9 MB, ...) |
| DEX | `classes.dex` 0.9 MB, `classes2.dex` 6.2 MB, `classes3.dex` 3.6 MB, 11,572 classes |
| Assets | 781 files, 73.9 MB: 458 png, 152 wav, 46+46 vsh/fsh, 9 mp4, 5 fnt, fonts, 2 html |
| Tools | readelf, llvm-objdump (armv7), dexdump, python+pyelftools; a custom parser for ObjC metadata (`__objc_classlist`, method lists, classrefs via `.rel.dyn` symbols) |

Native-to-Java usage was recovered three ways: (a) `Java_*` dynamic exports, (b) ObjC wrapper-class metadata in libBridgeKit and class references from other libs, (c) string tables (`__TEXT,__cstring`) holding the dotted Java class and member names registered via `+registerInstanceMethod:...`. Static scanning of JNIEnv function-table offsets was tried and was inconclusive (see 5.3).

## 2. App startup sequence (Java side, from dex)

Manifest: `com.apportable.app.VerdeApplication` + `com.apportable.activity.VerdeActivity` (portrait, `largeHeap`, GLES2 via meta-data `apportable.opengles2`), meta `android.app.libs` lists the 25 libs, `android.app.lib_name=Application`. Listed receivers/services (IAP, audio service, ad activities) are all removable.

1. `VerdeApplication.attachBaseContext` (MultiDex install, skippable) then `onCreate`: reflectively instantiates classes named in meta `apportable.app.lifecycle_listeners`, then `Lifecycle.onApplicationCreate`.
2. `VerdeActivity.onCreate`:
   - `AndroidAudioManager.getInstance(this).onCreate()`;
   - `Window.requestFeature/setFlags`, keep-screen-on flag;
   - creates `BackgroundLibraryLoader` and the root `FrameLayout`, shows `SplashScreen` (reads a splash asset);
   - **`MainThread.start()`** spawns a Java `Thread` named "MainThread" (custom stack size) that does `RunLoop.prepare()` (Looper.prepare + Handler + idle handler), `Looper.loop()`;
   - `startGame` runs `ExpansionFileDownloader.load` (no-op if there is no OBB), then `setContentView(root)`, then `BackgroundLibraryLoader.load`.
3. `BackgroundLibraryLoader` posts to the MainThread handler: `LibraryManager.loadLibraries(activity)`. This extracts/`System.load`s libs, calls `Library.resolveLinkages` (native, in libv, dependency order), `initializeLibv` and many `LibraryLoader.setenv(...)` calls: `DEBUG_BUILD`, `NSUnrecognizedForwardingDisabled`, `VERSION_CODE`, `LOCALE`, `LANGUAGES`, `TZ`, `PACKAGE_CODE_PATH`, `PACKAGE_RESOURCE_PATH`, `PACKAGE_NAME`, `HOME`, `TMPDIR`, `CACHEDIR`, `OBBDIR`, `ANDROID_*`. **A PC host loader replaces this whole step** by calling libv's `JNI_OnLoad` (it registers `LibraryLoader/Library` natives) or by just `setenv()` directly.
4. `Lifecycle` (a state machine with `sApplicationWasCreated`, `sActivityWasInited`, `sLibrariesWereLoaded`, `sContextHasSurface`) calls `testLaunched()`; when all flags are set it fires `startNativeApplication` ... which runs `MainThread.goNative()`, posting `nativeRun()` to the MainThread handler. **`Java_com_apportable_MainThread_nativeRun` calls `main -> UIApplicationMain` and never returns.**
5. Inside native, the CFRunLoop/NSRunLoop is *unified with the Android Looper* (`RunLoop.unified=true`): native calls `RunLoop.nativeNext(MessageQueue)` (a native replacement for `MessageQueue.next`) and `Handler.dispatchMessage`; `MainThread.tick()` runs tick actions and `RunLoop.processMessages()`. The fake Java layer therefore needs a working Looper/MessageQueue/Handler/Message with timed messages, `sendMessage`, `hasMessages`, idle handlers, `postDelayed`, and cross-thread posting (all native `dispatch` main-queue work goes through here).
6. GL: `VerdeApplication.getSurface()` returns `com.apportable.gl.GLSurfaceView` (a SurfaceView subclass with its own `EglHelper`, EGL10 config selection via `Utils.getEglConfigs/tryEglConfig`). On `readySurface`: create EGL context/surface, call renderer `onSurfaceCreated/onSurfaceChanged` which land in `GLSurfaceRenderer.surfaceCreated/surfaceChanged(J,GL10,...)`, then `Lifecycle.contextDidBecomeValid`. Draw cadence: `GLSurfaceRenderer.drawFrame(J,GL10)` per frame; `GLSurfaceView.swapBuffers`, `makeCurrentContext`, `createSharedContext`, `destroyContext` are *Java methods called back by native* (libApplication `VerdeActivity` class list: `getPrimaryContext makeCurrentContext createSharedContext destroyContext swapBuffers rektEGLContext setContentSize getContentWidth/Height getScreenWidth/Height`). On PC this is the natural seam: implement these on SDL/GLFW + ANGLE/GL ES.
7. Resume/foreground: `VerdeActivity.nativeOnCreate/Start/Resume`, `Lifecycle.applicationWillEnterForeground/DidBecomeActive`, `nativeRootGeometryChanged` (after layout), `gpgNativeOnActivity*` (Play Games / Noodle hooks, no-ops OK).

## 3. Native methods Java calls (Java -> native entry points)

196 `native` methods are declared in com.apportable/noodlecake classes (54 classes). 146 are bound by static `Java_*` exports (libApplication 90, libBridgeKit 77, minus overlap/undeclared; 167 `Java_*` symbols exist in total, 21 of them stale with no Java declaration: FlurryAdsShim x10, LocationListener x4, Screen x3, C2DM x1, RunLoop.nativeDispatchMessage, ui.View.notifyTouchEvent, ScrollView.notifyTouchStart). The remaining 50 have no static export and are bound with `RegisterNatives` (libv `JNI_OnLoad` for LibraryLoader/Library; BridgeKit `BridgeRegister*CallbackMethod` for listener/proxy classes such as MediaPlayer callbacks, AudioManager, Reachability, NoodleNews); `ReflectionProxy.invoke(JJObject,Object[])` is the generic dexmaker proxy trampoline.

Full table (bound via = library holding the `Java_*` symbol):

| Java class | native methods | bound via |
|---|---|---|
| `com/apportable/Accelerometer` | 1: onSensorChanged | RegisterNatives/proxy |
| `com/apportable/BlockRunnable` | 1: run | RegisterNatives/proxy |
| `com/apportable/DialogInterfaceOnCancelListener` | 1: nativeOnCancel | libBridgeKit.so |
| `com/apportable/DialogInterfaceOnClickListener` | 1: nativeOnClick | libBridgeKit.so |
| `com/apportable/GLSurfaceDispatch` | 1: AndroidGLSurfaceBindThread | libBridgeKit.so |
| `com/apportable/GLSurfaceRenderer` | 3: drawFrame, surfaceChanged, surfaceCreated | libBridgeKit.so |
| `com/apportable/JavaScriptInterface` | 1: nativeOnJavaScriptResult | libBridgeKit.so |
| `com/apportable/KeyHandler` | 4: nativeClearMetaKeyState, nativeOnKeyDown, nativeOnKeyOther, nativeOnKeyUp | libBridgeKit.so |
| `com/apportable/Library` | 1: resolveLinkages | RegisterNatives/proxy |
| `com/apportable/LibraryLoader` | 3: debugWait, init, setenv | RegisterNatives/proxy |
| `com/apportable/Lifecycle` | 8: applicationDidBecomeActive, applicationDidEnterBackground, applicationDidReceiveMemoryWarning, applicationWillEnterForeground, applicationWillResignActive, applicationWillTerminate, contextDidBecomeInvalid, contextDidBecomeValid | libApplication.so |
| `com/apportable/MainThread` | 1: nativeRun | libApplication.so |
| `com/apportable/ReflectionProxy` | 1: invoke | RegisterNatives/proxy |
| `com/apportable/SeekBarHandler` | 1: AndroidSeekBarEvent | libBridgeKit.so |
| `com/apportable/TextChangeHandler` | 3: AndroidTextChangeEvent, AndroidTextDidChangeEvent, AndroidTextWillChangeEvent | libBridgeKit.so |
| `com/apportable/VerdeOutputStream` | 4: nativeClose, nativeError, nativeFlush, nativeWrite | libBridgeKit.so |
| `com/apportable/VerdeResponseHandler` | 2: nativeAddHeader, nativeStatus | libBridgeKit.so |
| `com/apportable/ViewTouchEventHandler` | 2: nativeOnClick, nativeOnTouch | libBridgeKit.so |
| `com/apportable/WebViewClient` | 4: nativeOnLink, nativeOnPageFinished, nativeOnPageStarted, nativeOnReceivedError | libBridgeKit.so |
| `com/apportable/account/Callback` | 1: run | RegisterNatives/proxy |
| `com/apportable/activity/VerdeActivity` | 23: gpgNativeOnActivityCreated, gpgNativeOnActivityDestroyed, gpgNativeOnActivityPaused, gpgNativeOnActivityResult, gpgNativeOnActivityResumed, gpgNativeOnActivitySaveInstanceState, gpgNativeOnActivityStarted, gpgNativeOnActivityStopped, nativeHandleUri, nativeOnActivityResult, nativeOnConfigurationChanged, nativeOnCreate, nativeOnDestroy, nativeOnLowMemory, nativeOnNewIntent, nativeOnPause, nativeOnRestart, nativeOnResume, nativeOnSoftKeyboardChanged, nativeOnStart, nativeOnStop, nativeRootGeometryChanged, nativeSetEnv | RegisterNatives/proxy, libApplication.so |
| `com/apportable/cm/MotionManager` | 6: acceleration, gravity, gyroscope, magneticField, orientation, rotation | libApplication.so |
| `com/apportable/gl/GLSurfaceView` | 2: nativeInit, nativeOnSurfaceChanged | libApplication.so |
| `com/apportable/gl/SurfaceTexture` | 1: onFrameAvailable | RegisterNatives/proxy |
| `com/apportable/heyzapads/HeyzapAdsDelegateShim` | 1: nativePostNotification | libApplication.so |
| `com/apportable/iap/ApportableIAP` | 6: purchaseRequestDidError, purchaseStateDidChange, querySkusResponse, restorePurchasesCompleted, restorePurchasesDidError, setupFinished | libApplication.so |
| `com/apportable/media/AndroidAudioManager` | 3: audioFocusChange, resumeAudio, suspendAudio | RegisterNatives/proxy |
| `com/apportable/media/AudioPlayerItem` | 1: nativeOnComplete | libBridgeKit.so |
| `com/apportable/media/MediaPlayer` | 7: onBufferingUpdate, onCompletion, onError, onInfo, onPrepared, onSeekComplete, onVideoSizeChanged | RegisterNatives/proxy |
| `com/apportable/media/ScannerClient` | 2: onMediaScannerConnected, onScanCompleted | RegisterNatives/proxy |
| `com/apportable/net/Reachability` | 3: didStart, stateChangeFinished, stateChanged | RegisterNatives/proxy |
| `com/apportable/plusgames/PlusGamesManager` | 18: nativeLeaderboardsReceivedCallback, nativeOnConnected, nativeOnConnectionFailed, nativeOnDisconnected, nativeOnDisconnectedFromRoom, nativeOnInvitationAccepted, nativeOnInvitationReceived, nativeOnLeftRoom, nativeOnPeerJoined, nativeOnPeerLeft, nativeOnPeersConnected, nativeOnPeersDisconnected, nativeOnRealTimeMessageReceived, nativeRoomConnectedCallback, nativeScoresReceivedCallback, nativeSelectPlayersFailed, nativeUserAchievementsReceivedCallback, nativeWaitingRoomResult | libApplication.so |
| `com/apportable/ui/AlertView` | 3: nativeOnCancel, nativeOnClick, nativeOnDismiss | libBridgeKit.so |
| `com/apportable/ui/BarButtonItem` | 1: click | libBridgeKit.so |
| `com/apportable/ui/Button` | 1: clicked | RegisterNatives/proxy |
| `com/apportable/ui/Control` | 1: event | libBridgeKit.so |
| `com/apportable/ui/Device` | 1: nativeOnOrientationChanged | libBridgeKit.so |
| `com/apportable/ui/MediaPlayer` | 1: playbackCompleted | libBridgeKit.so |
| `com/apportable/ui/PickerView` | 4: numberOfComponents, numberOfRowsInComponent, rowHeightForComponent, widthForComponent | libBridgeKit.so |
| `com/apportable/ui/PickerViewCell` | 3: cellAttached, cellDetached, cellForRowInComponent | libBridgeKit.so |
| `com/apportable/ui/ScrollView` | 4: notifyAutoScrollFinished, notifyFling, notifyScroll, notifyTouchFinish | libBridgeKit.so |
| `com/apportable/ui/TextField` | 11: actionBarWillHide, actionBarWillShow, didBeginEditing, didEndEditing, shouldBeginEditing, shouldChangeCharactersInRange, shouldClear, shouldEndEditing, shouldReturn, textDidChange, textWillChange | RegisterNatives/proxy, libBridgeKit.so |
| `com/apportable/ui/TextView` | 7: actionBarWillHide, actionBarWillShow, didBeginEditing, selectionChange, textDidChange, textShouldChange, textWillChange | RegisterNatives/proxy, libBridgeKit.so |
| `com/apportable/ui/View` | 5: callDrawRect, callLayoutSubviews, getDescription, isDrawRectImplemented, nativeSetFrame | libBridgeKit.so |
| `com/apportable/ui/WebDialog` | 2: onClose, onOverrideURL | RegisterNatives/proxy |
| `com/apportable/ui/WebView` | 4: onLink, onPageFinished, onPageStarted, onReceivedError | libBridgeKit.so |
| `com/apportable/ui/Window` | 8: nativeKeyDown, nativeKeyUp, nativeOnWindowFocusChanged, nativeTouchesBegin, nativeTouchesCancel, nativeTouchesEnd, nativeTouchesMove, motionEvent | RegisterNatives/proxy, libApplication.so |
| `com/apportable/utils/RunLoop` | 1: nativeNext | libApplication.so |
| `com/apportable/utils/Single7z` | 1: extract | RegisterNatives/proxy |
| `com/noodlecake/noodlenews/NoodleNewsClient$Debug` | 10: nativeAcknowledgeSupportResponse, nativeDismissedNativeCreative, nativeGetNativeCreative, nativeGetSupportIdentifier, nativeGetSupportResponseContent, nativeGetSupportResponseID, nativeHasPendingCreative, nativeHitNativeCreative, nativeShowCreative, nativeShowMoreGames | RegisterNatives/proxy |
| `com/noodlecake/noodlenews/NoodleNewsClient` | 5: nativeDidDismissCreative, nativeDidDismissMoreGames, nativeInitialized, nativeWillShowCreative, nativeWillShowMoreGames | RegisterNatives/proxy |
| `com/noodlecake/noodlenewsapportable/NoodleNewsClientApportableDelegate` | 4: nativeDidDismissCreative, nativeDidDismissMoreGames, nativeWillShowCreative, nativeWillShowMoreGames | libApplication.so |
| `com/noodlecake/noodlewebview/BlockheadsWebView` | 1: nativeSetWelcomeMessage | libApplication.so |
| `com/noodlecake/utils/NoodlePermissionGranter` | 1: nativePermissionGrantedResult | libApplication.so |

### 3.1 Which entry points are needed

| Tier | Entry points | Count |
|---|---|---|
| **Boot to main menu** | `JNI_OnLoad` (libv, libBridgeKit); `VerdeActivity.nativeSetEnv` (or direct `setenv`); `MainThread.nativeRun`; `VerdeActivity.nativeOnCreate/OnStart/OnResume/nativeRootGeometryChanged`; `Lifecycle.contextDidBecomeValid/applicationWillEnterForeground/applicationDidBecomeActive`; `GLSurfaceView.nativeInit/nativeOnSurfaceChanged`; `GLSurfaceRenderer.surfaceCreated/surfaceChanged/drawFrame`; `GLSurfaceDispatch.AndroidGLSurfaceBindThread`; `RunLoop.nativeNext`; `Window.nativeTouchesBegin/Move/End/Cancel`, `nativeOnWindowFocusChanged` | ~28 |
| **Full gameplay (adds)** | pause/resume/stop/destroy lifecycle (9); `Window.nativeKeyDown/Up`, `KeyHandler.*` (4), `TextField.*` (11) and `TextView.*` (7) for chat text entry; `AlertView`/`DialogInterface*` (5); `VerdeOutputStream.*` (4), `VerdeResponseHandler.*` (2) for HTTP bodies/headers; `MediaPlayer.on*` (7), `AudioManager.*` (3), `AudioPlayerItem.nativeOnComplete`; `Reachability.*` (3); `ui.View.*` (5), `ScrollView` (4) where UIKit views are used | ~55 |
| **Stub / never call** | IAP (6), PlusGames (18), NoodleNews (15), Flurry/Heyzap (11+), MotionManager/Accelerometer (7, optional sensors), WebView/WebDialog/JavaScriptInterface (~14), C2DM, Single7z (1), gpgNative* (8), PickerView (7), Location (4) | ~90 |

Parameters of note: most handlers carry a `jlong` native-object pointer as their first argument (`nativeOnTouch(J,View,MotionEvent)`, `KeyHandler.*`, `GLSurfaceRenderer.*(J,GL10,...)`), so the host must keep that pointer exactly as native handed it to Java at registration time (it is passed from native to Java constructors, e.g. the `VerdeGLSurfaceRenderer` init).

## 4. Java methods native calls (native -> Java) by subsystem

### 4.1 The bridge itself
`libv.so` `JNI_OnLoad` caches the JavaVM; BridgeKit exposes `AndroidGetJavaVM`, `AndroidGetJavaEnv`, `AndroidFindClass`, `AndroidClassLoader` and uses a global-ref table (`_BridgeReference{Add,Find,Delete}`). `BridgeRegister{Instance,Nonvirtual,Class,Static,Callback,StaticCallback}Method`, `...Field`, `...Constructor`, `...Stub*` are the registration entry points. It also uses **`com.google.dexmaker`** (DexMaker/Code/MethodId/TypeId/Local, present in classes3.dex) to *generate Java classes at runtime* (Java subclasses/proxies of listener interfaces whose methods land in native through `ReflectionProxy`). A fake JVM can short-circuit this by creating host-side proxy classes without bytecode; the DexMaker API surface then has to be faked (about 5 classes, ~15 methods) or the bridge patched at the 5 call sites.

### 4.2 Wrapper classes actually referenced by game/UIKit code
libApplication references **70** wrapper classes directly (48 non-UI, 22 `VerdeUI*`); libAudioToolbox/AudioUnit/AudioFile add `AndroidAudioManager` and `AndroidActivity`. Their 376 wrapper methods intersect libApplication/Foundation/CoreFoundation selector refs in 281 cases (209 non-UI), an upper-bound proxy for "Java calls that can happen".

Non-UI classes (selector hits / methods): `JavaObject` 31/68 and `JavaClass` 11/27 (the reflection base: allocation, typecast, registerXxx), `AndroidContext` 10/15, `AndroidAudioManager` 7/10, `AndroidBitmap` 7/9, `AndroidIntent` 8/12, `AndroidMediaPlayer` 7/15, `AndroidSurfaceTexture` 7/8, `AndroidBundle` 7/7, `AndroidActivity` 6/8, `AndroidBuild` 6/6 + `AndroidBuildVersion` 4/4, `AndroidMediaScannerConnection` 6/6, `AndroidAudioPlayer` 4/4, `AndroidView` 4/7, `AndroidSharedPreferences(+Editor)` 3+4, `AndroidEnvironment` 3, `AndroidSecureSettings` 3 (androidId), `AndroidHandler/Looper/Message/Runnable/Thread/System` (small), `AndroidHttpRequest` 3/5 plus `AndroidHttpMessageShim`, collections (`JavaArrayList/LinkedHashMap/List/Set/SoftReference/GregorianCalendar/ByteBuffer/File/Boolean`), `AndroidEGLContext` 3/4, `AndroidSurface`, `AndroidTypeface`, `AndroidMatrix`, `AndroidURI`, `AndroidClipData`, `AndroidLocationManager`.
UI classes (`VerdeUI*`: View, Window, Label, Button, TextField, TextView, ScrollView, ImageView, AlertView, Switch, Slider, PickerView, ProgressView, NavigationBar, Toolbar, BarItem/BarButtonItem, WebView, MediaPlayer, LocalNotification, Device, ActivityIndicator): 94 hits. For a GL game most of these are only instantiated by dialogs/text input; `VerdeUIWindow`, `VerdeUIView`, `VerdeUIDevice`, `VerdeUITextField/TextView` (chat), `VerdeUIAlertView` are the realistic ones. Note UIKit lives in libGraphicsServices/libApplication and creates a Java `Window`/`View` hierarchy even for a GL-only game; the fake layer must at least accept `ui.Window` and `ui.View` creation, `setFrame`, `addSubview`, visibility.

### 4.3 Game/shim classes bridged from libApplication (88 dotted Java names)
- Platform: `VerdeActivity` (canSendEmail, sendEmail, getPrimaryContext, makeCurrentContext, createSharedContext, destroyContext, savePreferenceEditorHack, setContentSize, getContent/Screen/SplashScreen Width/Height, getAttributionId, homeScreen, rektEGLContext, androidId, hasSystemFeature, hasActivityForAction, gc, uuid, swapBuffers, orientation get/set), `MainThread.tick`, `Notifications` (8: post, schedule/cancel/persist local notifications, ...), `Reachability` (start, stateChanged...), `ui.Device`, `Accelerometer`, `cm.MotionManager` (28 selectors), `android.widget.Toast` (makeText, show), `android.content.Context`.
- Billing: `ApportableIAP` (purchaseObject, confirmNotification, consumePurchase, isBillingSupported, areSubscriptionsSupported, restoreTransactions, querySkus = 7) with 6 native callbacks (`setupFinished`, `querySkusResponse`, `purchaseStateDidChange`, ...). **Stub as "billing unavailable"**; game falls back (RESULT_BILLING_UNAVAILABLE error domain exists).
- Ads/analytics (all optional): `FlurryAgent` 9, `FlurryAds` 9, `FlurryAdsShim`, `FlurryAnalyticsShim`, `HeyzapAds` (startListeningForCallbacks), `HeyzapAdsBannerShim/SafeShowShim/DelegateShim`, `InterstitialAd` (display, fetch, isAvailable), `IncentivizedAd.safeShow`, `VideoAd`, `AdvertisingIdClient(.Info)` (getAdvertisingIdInfo, getId, isLimitAdTrackingEnabled). Stub to "no ad available".
- Social/meta: `PlusGamesManager` (30 methods + 18 natives; Google Play Games sign-in, achievements, leaderboards, real-time rooms), `NoodleNewsClient(+Debug, INoodleNewsClientDelegate, Creative)` (~12 methods + 15 natives), `NoodleWebView`, `NoodlePermissionGranter`. All stubbable as "signed out / nothing pending".
- Files and storage: the Java side exposes `AndroidSharedPreferences` (NSUserDefaults backing: `toDictionary`), `AndroidEnvironment.externalStoragePublicDirectory:`, `Context.getFilesDir/getCacheDir/getExternalFilesDir/getFileStreamPath` (these resolve the game's save directory), `AndroidUbiquitousKeyValueStoreProvider` (iCloud KVS emulation over prefs). Game save/worlds/name/keychain live in files and prefs (SFHFKeychainUtils is in libApplication).

### 4.4 Counting the registered Java surface (BridgeKit string tables)
`__TEXT,__cstring` of libBridgeKit (37 KB) holds the dotted class names and member names: 498 distinct class names (`android.*`, `java.*`, `javax.microedition.khronos.egl.*`, `com.apportable.*`, 6 `com.google.dexmaker.*`) and 1,275 distinct member/property tokens (includes ObjC property names, so it overestimates Java methods; roughly 900-1,000 are real Java methods/fields). The 70 directly-referenced wrapper classes map to ~510 member tokens. Classes not touched by this game (Camera x6, Geocoder, Wifi, Nsd, Account manager, Animation set, Vibrator, TelephonyManager ...) can be ignored: member lookups happen when a class is first used (`+initializeJava`), so a fake whose `GetMethodID` returns a dummy ID for any unknown member (and logs it) boots fine as long as the call is never *made*.

## 5. Estimating the fake JNI environment

### 5.1 What the host must provide at the JNIEnv level
Whatever else is chosen, a `JNIEnv`/`JavaVM` function table of 233 slots is required (`GetEnv/AttachCurrentThread/DetachCurrentThread` on `JavaVM`). Expected real implementations (observed directly in BridgeKit/libv: `FindClass`, `GetMethodID`, `NewGlobalRef`, `DeleteLocalRef`, `ExceptionDescribe`, `DefineClass`, `RegisterNatives`, `GetStaticMethodID`, `GetFieldID`; the invocation of `Call<Type>Method[V/A]`, `CallStatic<Type>Method`, `CallNonvirtual<Type>Method`, `Get/Set<Type>Field`, `NewObject*` is done through generated stubs, not a fixed code pattern):
- Reflection: FindClass, GetObjectClass, GetSuperclass, IsInstanceOf, IsAssignableFrom, GetMethodID/GetStaticMethodID, GetFieldID/GetStaticFieldID, ToReflected*/FromReflected* (maybe), DefineClass (dexmaker, can fail).
- Calls: Call{Void,Object,Boolean,Int,Long,Float,Double,Byte,Short,Char}Method{,V,A} (+Static +Nonvirtual) = 90 slots, but a single dispatcher can serve all.
- Fields: Get/Set{...}Field and Static (36 slots).
- Objects/refs: New/Delete Local/Global/WeakGlobalRef, PushLocalFrame, EnsureLocalCapacity, IsSameObject, AllocObject, NewObject*.
- Strings: NewStringUTF, GetStringUTFChars/Release, GetStringLength, GetStringChars/NewString, GetStringUTFLength, Region variants.
- Arrays: GetArrayLength, New{Object,Byte,Int,Float,...}Array, Get/Release<T>ArrayElements, Get/Set<T>ArrayRegion, Get/SetObjectArrayElement, Get/ReleasePrimitiveArrayCritical.
- Exceptions: Throw, ThrowNew, ExceptionCheck/Occurred/Describe/Clear. (BridgeKit has a `JavaThrowableLandingPad`; exceptions must at least be detectable and clearable.)
- Misc: MonitorEnter/Exit, GetJavaVM, RegisterNatives, NewDirectByteBuffer/GetDirectBufferAddress/Capacity (OpenAL AudioTrack path uses `byte[]`, `ByteBuffer` for bitmaps), GetVersion.

Approximately **60-70 of the 233 slots** need working bodies; the rest can be `abort()`/log stubs.

### 5.2 What the host must provide at the Java-object level (the larger cost)
This is a mini JVM-less "Java" model, with host-implemented classes registered as `(class, name, sig) -> closure`.

| Level | Java classes (approx.) | (class,member) entries resolved | Entries with real behaviour |
|---|---|---|---|
| **Minimal boot to menu, touch only** | ~45 | 130-180 | 40-60 |
| **Full gameplay (single player + LAN/online ENet, HTTP, audio, chat text input, save, rate/share dialogs)** | ~90 | 350-450 | 120-160 |
| Everything (all 498 registered classes) | 498 | ~1,000-1,275 | n/a, not needed |

Minimal-boot class set (judgement from section 2/4, to be confirmed with a call trace): `VerdeActivity`, `Context/ContextWrapper`, `Activity`, `Build`, `Build$VERSION`, `Bundle`, `SharedPreferences(+Editor)`, `Environment`, `File`, `System`, `Thread`, `Looper/Handler/Message/MessageQueue/IdleHandler`, `Runnable`, `ArrayList/HashMap/LinkedHashMap/List/Set/Boolean/Integer/...`, `ClassLoader/Class`, `FrameLayout/ViewGroup/View/SurfaceView/Surface/SurfaceHolder`, `GLSurfaceView`, `EGL10/EGLContext/EGLDisplay/EGLSurface/EGLConfig`, `Display/DisplayMetrics/WindowManager/Window`, `Settings.Secure` (androidId), `AssetManager/Resources` (fonts and splash), `Typeface`, `ui.Window/ui.View/ui.Device`, `Locale/TimeZone/Calendar`, `UUID`, plus the stub shims: IAP, Reachability, Notifications, PlusGames, NoodleNews.

Added for gameplay: `HttpClient` family (`DefaultHttpClient`, `HttpPost/Get`, `HttpResponse`, `HttpEntity`, `ByteArrayEntity`, header APIs, `HttpMessageShim`, `VerdeResponseHandler`, `VerdeOutputStream`, `java.net.URL/URLConnection` seen too), `MediaPlayer` (+listeners, 9 mp4 music tracks) or a decode-to-OpenAL bypass, `AudioTrack` (OpenAL fallback), `AudioManager`, `ClipboardManager`, `Intent` (open URL/email), `Toast`, `AlertDialog`/`ui.AlertView`, `ui.TextField/TextView` + `KeyHandler`/`TextChangeHandler`, `InputMethod` (soft keyboard events via `nativeOnSoftKeyboardChanged`), `Bitmap` and `BitmapFactory` (only if UIKit image views are used), `ConnectivityManager/NetworkInfo`.

### 5.3 Confidence and what would firm these numbers up
- Direct JNI call counts could not be recovered statically: Call*/Get*Field are dispatched through BridgeKit stubs generated per registered member, not fixed `ldr/blx` patterns (`bridge_msgSend` is a 7-instruction trampoline). Counts above are therefore bounded by the wrapper/selector inventory, not call-site counts. The honest error bars are about +/-40%.
- A **runtime call trace** is the correct next step. The fake-JNI layer should log every `FindClass/GetMethodID/Call*` (class, name, sig) and be iterated until the menu appears. Because names/signatures are passed as plain C strings at runtime, the trace is complete and cheap; expect one afternoon of iteration per major subsystem.
- Unknown: whether `Class.forName("android.support.multidex.MultiDex")`, `ExpansionFileDownloader` and `SplashScreen` need faking (we can skip all three by not running the Java startup code at all).

## 6. Subsystem table: what the fake Java must do

| Subsystem | Java used | Needed for menu | Needed for gameplay | Host implementation |
|---|---|---|---|---|
| Lifecycle / threading | `MainThread`, `RunLoop`, Looper/Handler/Message | yes | yes | Host event loop with timed message queue; `nativeRun` on a dedicated big-stack thread; implement `RunLoop.nativeNext` contract (it is a *native* function in the game libs: host only has to feed `Message` objects) |
| Library loading | `LibraryLoader`, `Library`, `LibraryManager` | replaced | replaced | Own ELF loader + `setenv`; call `Library.resolveLinkages` equivalent ourselves |
| Surface / EGL | `GLSurfaceView`, `EglHelper`, `Surface`, EGL10 | yes | yes | SDL/GLFW window + GL ES via ANGLE or desktop GL shim; implement `swapBuffers/makeCurrentContext/createSharedContext/destroyContext/setContentSize/getScreenWidth...`; renderer callbacks call `GLSurfaceRenderer.*` natives with a fake `GL10` jobject |
| Touch input | `ui.Window.nativeTouches*`, `ViewTouchEventHandler`, `MotionEvent` | yes | yes | Map mouse to `nativeTouchesBegin/Move/End` (float coords, pointer id arrays for move) |
| Keyboard | `Window.nativeKeyDown/Up`, `KeyHandler`, `TextField` natives, `InputMethodManager` | no | chat | Map SDL text input to `TextField.*` delegate callbacks; hardest UIKit-in-Java piece |
| Files / assets | libv zzip + `__wrap_*`; `Context.getFilesDir/...` | yes | yes | Host: set env vars to APK path, home/cache dirs; provide `__wrap_*` shims = host libc. No Java for assets. |
| Preferences | `SharedPreferences(+Editor)` | yes | yes | JSON/INI file; `toDictionary` returns Map |
| Networking (UDP/ENet) | libc sockets | no | yes | Host sockets, nothing Java |
| HTTP | `HttpClient`, `VerdeResponseHandler`, `VerdeOutputStream` | no (probably runs at start for NoodleNews/IAP check, can fail) | yes | libcurl/WinHTTP wrapper presenting the 20-30 Apache methods |
| Audio | OpenAL (native) -> OpenSL ES (`dlopen libOpenSLES.so`) or `AudioTrack.write([BII)` fallback; `AudioManager`; `MediaPlayer` for mp4 music | optional | yes | Provide `AudioTrack` or a fake libOpenSLES -> SDL audio / miniaudio; mp4(AAC) music decode needs host AAC decoder (FFmpeg/minimp4+fdk or stub silence) |
| Device info | `Build`, `DisplayMetrics`, `Settings.Secure.ANDROID_ID`, `Configuration` | yes | yes | Return PC-synthesized values (density, size) |
| IAP | `ApportableIAP` | yes (initialization) | yes | `isBillingSupported = false`; fire `setupFinished` |
| Ads / analytics | Flurry, Heyzap | no | no | no-ops; `isAvailable=false` |
| Play Games / NoodleNews | `PlusGamesManager`, `NoodleNewsClient` | init only | no | `isLoggedIn=false`; `hasPendingCreative=false` |
| Notifications / reachability | `Notifications`, `Reachability` | init | rare | no-op; reachability always "WiFi" |
| Sensors | `MotionManager`, `Accelerometer` | no | no | report unavailable |

## 7. Conclusions

- The Java surface is **moderate and tractable**: about 28 entry points and 130-180 member IDs for a main-menu boot; hundreds, not thousands, for gameplay.
- Biggest risks inside this scope:
  1. The **BridgeKit reflection design** (runtime-built signatures, dexmaker proxies, `JavaThrowableLandingPad` exception handling) makes a faithful fake JNIEnv more work than a tidy API would be. Alternative: replace libBridgeKit with a hand-written host library exporting the same ObjC wrapper classes (the 70 referenced classes, ~376 methods, no JNI at all); only viable if our ObjC runtime path (libSystem objc) is already working, but it removes JNI/DexMaker entirely.
  2. The **unified Looper/CFRunLoop** (`RunLoop.nativeNext`): the native run loop blocks on a Java MessageQueue, so we must emulate Looper semantics precisely (wake-ups, idle handlers, timers).
  3. **UIKit-over-Java widgets** (`ui.Window/View/TextField`): UIKit in libGraphicsServices/libApplication mirrors every `UIView` as a Java object, so a host must at least accept those calls even for a pure-GL game.
  4. **Audio** (OpenSL `dlopen` + AudioTrack fallback + AAC mp4 music) and **HTTP** (Apache HttpClient shims) are the only two subsystems that require real functionality beyond stubs for gameplay; IAP/ads/Play Games/NoodleNews are pure stubs.
- Nothing here blocks the approach. Recommended build order: (1) fake JavaVM with trace logging, (2) MainThread/Looper, (3) EGL/surface + renderer natives, (4) touch, (5) prefs/files/Build, (6) HTTP, (7) audio, (8) text input.

## Appendix: reproduction notes
Scratch files (not distributed): `/tmp/claude-1000/-home-kahverengi-Projects-reverse-engineering/01f1a716-a62a-46f9-ac38-555fce897d14/scratchpad/pcport/b/` contains `dexclasses.json` (per-class method/native lists), `wrappers.json` (BridgeKit ObjC wrapper methods), `classrefs.json`, `bk_members.json` (Java members per dotted class), `exports.json`, `app_cstrings.txt`.
