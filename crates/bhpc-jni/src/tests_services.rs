//! Billing, Play Games, the host's messages and the picture picker (`jlib::services`, `jlib::intent`).

use crate::tests::Env;
use crate::{file_uri, uri_path, ActivityRequest, Obj, Value, RESULT_CANCELED, RESULT_OK};

const IAP: &str = "com/apportable/iap/ApportableIAP";
const GAMES: &str = "com/apportable/plusgames/PlusGamesManager";

/// A native that records r2, r3, `[sp]` and `[sp + 12]` at `slot` (4 words) and returns.
fn recorder(e: &mut Env) -> (u32, u32) {
    let slot = e.buf(16);
    let code = e.code(&[
        0xE59F_C018, // ldr r12, [pc, #24]  (the slot address below)
        0xE58C_2000, // str r2, [r12]
        0xE58C_3004, // str r3, [r12, #4]
        0xE59D_0000, // ldr r0, [sp]
        0xE58C_0008, // str r0, [r12, #8]
        0xE59D_000C, // ldr r0, [sp, #12]
        0xE58C_000C, // str r0, [r12, #12]
        0xE12F_FF1E, // bx lr
        slot,
    ]);
    (code, slot)
}

fn words(e: &Env, slot: u32) -> [u32; 4] {
    let m = e.mem();
    [0, 4, 8, 12].map(|o| m.read_u32(slot + o).unwrap())
}

fn calls(e: &Env, class: &str, name: &str) -> u64 {
    e.vm.report().members.iter().filter(|m| m.class == class && m.name == name).map(|m| m.calls).sum()
}

fn pump(e: &mut Env) -> usize {
    let vm = e.vm.clone();
    let now = vm.now_ms();
    vm.pump(&mut e.cpu, vm.main_looper(), now).unwrap()
}

fn bind(e: &mut Env, class: &str, name: &str, sig: &str) -> u32 {
    // The recorder reads stack words even when a call has none: keep mapped memory above the stack pointer.
    e.cpu.set_sp(crate::tests::STACK_TOP - 256);
    let (code, slot) = recorder(e);
    let k = e.vm.class_named(class).unwrap();
    assert!(e.vm.register_native(k, name, sig, code));
    slot
}

fn iap(e: &mut Env) -> Obj {
    e.new_obj(IAP, "(JZ)V", &[Value::Long(0x1_0000_00AB), Value::Int(1)])
}

#[test]
fn billing_reports_itself_unavailable_and_never_buys() {
    let mut e = Env::new();
    let setup = bind(&mut e, IAP, "setupFinished", "(J)V");
    let o = iap(&mut e);
    assert_eq!(calls(&e, IAP, "setupFinished"), 0, "the setup ends later, on the main looper");
    assert_eq!(pump(&mut e), 1);
    assert_eq!(calls(&e, IAP, "setupFinished"), 1);
    assert_eq!(words(&e, setup)[..2], [0xAB, 1], "with the native peer");
    for name in ["isBillingSupported", "areSubscriptionsSupported", "restoreTransactions"] {
        assert_eq!(e.call(o, name, "()Z", &[]).unwrap(), Value::Int(0), "{name}");
    }
    let (a, b, cc) = (e.string("crystalsage"), e.string("inapp"), e.string("42"));
    let sig = "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Z";
    assert_eq!(e.call(o, "purchaseObject", sig, &[a, b, cc]).unwrap(), Value::Int(0));
    let s = e.string("x");
    assert_eq!(e.call(o, "consumePurchase", "(Ljava/lang/String;)Z", &[s]).unwrap(), Value::Int(0));
    assert_eq!(e.call(o, "confirmNotification", "(Ljava/lang/String;I)V", &[s, Value::Int(1)]).unwrap(), Value::Void);
    let sig = "(IILandroid/content/Intent;)Z";
    assert_eq!(
        e.call(o, "handleActivityResult", sig, &[Value::Int(1), Value::Int(2), Value::Ref(None)]).unwrap(),
        Value::Int(0)
    );
    // A product query is answered with an error at once (Android would leave it unanswered).
    let response = bind(&mut e, IAP, "querySkusResponse", "(JJLjava/lang/String;Z)V");
    let skus = e.string("crystalsage,crystalsera");
    let r = e.call(o, "querySkus", "(JLjava/lang/String;)Z", &[Value::Long(77), skus]).unwrap();
    assert_eq!(r, Value::Int(1));
    assert_eq!(pump(&mut e), 1);
    let w = words(&e, response);
    assert_eq!((w[0], w[1], w[2], w[3]), (0xAB, 1, 77, 1), "peer, request id and the error flag");
    assert!(e.vm.take_notices().is_empty(), "no message before the user touched the game");
    assert!(e.log_text().contains("in-app billing: unavailable"));
}

#[test]
fn the_store_and_achievements_tell_the_user_once_they_touch_the_game() {
    let mut e = Env::new();
    let o = iap(&mut e);
    e.vm.note_user_input();
    let skus = e.string("crystalsage");
    e.call(o, "querySkus", "(JLjava/lang/String;)Z", &[Value::Long(1), skus]).unwrap();
    assert!(e.vm.take_notices().is_empty(), "the game's own product queries stay quiet");
    let g = e.new_obj(GAMES, "(ILandroid/app/Activity;)V", &[Value::Int(9), Value::Ref(None)]);
    e.call(g, "showAchievements", "()V", &[]).unwrap();
    let id = e.string("board");
    e.call(g, "showLeaderboard", "(Ljava/lang/String;)V", &[id]).unwrap();
    assert_eq!(e.vm.take_notices(), [crate::GAMES_NOTICE]);
    assert!(e.vm.take_notices().is_empty());
    let (a, b, c) = (e.string("p"), e.string("t"), e.string("r"));
    let sig = "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Z";
    e.call(o, "purchaseObject", sig, &[a, b, c]).unwrap();
    e.call(o, "purchaseObject", sig, &[a, b, c]).unwrap();
    assert_eq!(e.vm.take_notices(), [crate::STORE_NOTICE], "a waiting message is not repeated");
}

#[test]
fn view_achievements_is_told_by_its_lone_sign_in_check() {
    let mut e = Env::new();
    let o = iap(&mut e);
    let g = e.new_obj(GAMES, "(ILandroid/app/Activity;)V", &[Value::Int(9), Value::Ref(None)]);
    let step = |e: &mut Env, ms: u64| e.clock.fetch_add(ms, std::sync::atomic::Ordering::Relaxed);
    // Before any input (start-up): quiet.
    e.call(g, "isLoggedIn", "()Z", &[]).unwrap();
    step(&mut e, crate::GAMES_CHECK_MS);
    assert!(e.vm.take_notices().is_empty());
    e.vm.note_user_input();
    // Back to the main menu: the sign-in check comes with a product query.
    e.call(g, "isLoggedIn", "()Z", &[]).unwrap();
    let skus = e.string("crystalsage");
    e.call(o, "querySkus", "(JLjava/lang/String;)Z", &[Value::Long(1), skus]).unwrap();
    step(&mut e, crate::GAMES_CHECK_MS);
    assert!(e.vm.take_notices().is_empty());
    // "View achievements": the check alone, told once its time is up.
    e.call(g, "isLoggedIn", "()Z", &[]).unwrap();
    assert!(e.vm.take_notices().is_empty(), "not yet");
    step(&mut e, crate::GAMES_CHECK_MS);
    assert_eq!(e.vm.take_notices(), [crate::GAMES_NOTICE]);
    assert!(e.vm.take_notices().is_empty());
}

#[test]
fn a_touch_counts_as_user_input() {
    let mut e = Env::new();
    let vm = e.vm.clone();
    vm.ui_touch(&mut e.cpu, crate::TouchPhase::Begin, 1.0, 1.0).unwrap();
    let g = e.new_obj(GAMES, "(ILandroid/app/Activity;)V", &[Value::Int(9), Value::Ref(None)]);
    e.call(g, "connect", "(Landroid/view/View;)V", &[Value::Ref(None)]).unwrap();
    assert_eq!(e.vm.take_notices(), [crate::GAMES_NOTICE]);
}

#[test]
fn play_games_sign_in_fails_like_a_device_without_play_services() {
    let mut e = Env::new();
    let failed = bind(&mut e, GAMES, "nativeOnConnectionFailed", "(II)V");
    let g = e.new_obj(GAMES, "(ILandroid/app/Activity;)V", &[Value::Int(0x55), Value::Ref(None)]);
    for name in ["isLoggedIn", "isAchievementsLoaded", "getShouldShowInvitationInbox"] {
        assert_eq!(e.call(g, name, "()Z", &[]).unwrap(), Value::Int(0), "{name}");
    }
    e.call(g, "connect", "(Landroid/view/View;)V", &[Value::Ref(None)]).unwrap();
    assert!(e.vm.take_notices().is_empty(), "the sign-in at start-up stays quiet");
    assert_eq!(pump(&mut e), 1);
    assert_eq!(words(&e, failed)[..2], [0x55, crate::SERVICE_MISSING as u32]);
    assert!(e.log_text().contains("sign-in fails"));
}

#[test]
fn a_callback_runs_once_and_its_errors_reach_the_looper() {
    let mut e = Env::new();
    // Without a bound native the call is a reported stub; a callback object that lost its call does nothing.
    let o = iap(&mut e);
    let _ = o;
    let k = e.vm.find_class("com/apportable/HostCallback").unwrap();
    let stray = e.vm.alloc_object(k);
    e.vm.set_field(stray, "id", Value::Int(999));
    assert_eq!(e.call(stray, "run", "()V", &[]).unwrap(), Value::Void);
    assert_eq!(pump(&mut e), 1);
    assert_eq!(calls(&e, IAP, "setupFinished"), 1);
    assert_eq!(pump(&mut e), 0);
    // An instance callback on a receiver that went away throws into the looper.
    let g = e.new_obj(GAMES, "(ILandroid/app/Activity;)V", &[Value::Int(1), Value::Ref(None)]);
    e.call(g, "connect", "(Landroid/view/View;)V", &[Value::Ref(None)]).unwrap();
    e.vm.st().services.deferred.values_mut().for_each(|d| d.recv = Some(Obj(0xFFFF)));
    let vm = e.vm.clone();
    let now = vm.now_ms();
    assert!(vm.pump(&mut e.cpu, vm.main_looper(), now).is_err());
}

#[test]
fn file_uris_round_trip() {
    assert_eq!(file_uri("/a/ü %.png"), "file:///a/%C3%BC%20%25.png");
    assert_eq!(uri_path("file:///a/%C3%BC%20%25.png").as_deref(), Some("/a/ü %.png"));
    assert_eq!(uri_path("/plain/path").as_deref(), Some("/plain/path"));
    assert_eq!(uri_path("file:///bad%zz%4").as_deref(), Some("/bad%zz%4"), "broken escapes stay as they are");
    assert_eq!(uri_path("content://x"), None);
}

fn intent(e: &mut Env, action: &str, data: Option<&str>) -> Obj {
    let a = e.string(action);
    match data {
        Some(d) => {
            let s = e.string(d);
            let u = e.scall("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[s]).unwrap();
            e.new_obj("android/content/Intent", "(Ljava/lang/String;Landroid/net/Uri;)V", &[a, u])
        }
        None => e.new_obj("android/content/Intent", "(Ljava/lang/String;)V", &[a]),
    }
}

#[test]
fn the_picture_picker_request_reaches_the_host_and_its_answer_the_game() {
    let mut e = Env::new();
    let i = intent(&mut e, "android.intent.action.PICK", Some("content://media/internal/images/media"));
    let act = e.vm.activity();
    let sig = "(Landroid/content/Intent;I)V";
    e.call(act, "startActivityForResult", sig, &[Value::Ref(Some(i)), Value::Int(9001)]).unwrap();
    e.call(act, "startActivityForResult", sig, &[Value::Ref(None), Value::Int(1)]).unwrap();
    let reqs = e.vm.take_activity_requests();
    assert_eq!(
        reqs,
        [ActivityRequest {
            code: 9001,
            action: "android.intent.action.PICK".into(),
            data: Some("content://media/internal/images/media".into()),
            mime: None,
        }]
    );
    assert!(reqs[0].wants_image());
    assert!(e.vm.take_activity_requests().is_empty());
    let result = bind(
        &mut e,
        "com/apportable/activity/VerdeActivity",
        "nativeOnActivityResult",
        "(IILandroid/content/Intent;)V",
    );
    let vm = e.vm.clone();
    vm.activity_result(&mut e.cpu, 9001, RESULT_OK, Some("file:///tmp/p.png")).unwrap();
    let w = words(&e, result);
    assert_eq!((w[0], w[1] as i32), (9001, RESULT_OK));
    assert_ne!(w[2], 0, "an intent with the picture's URI");
    vm.activity_result(&mut e.cpu, 9001, RESULT_CANCELED, None).unwrap();
    let w = words(&e, result);
    assert_eq!((w[1] as i32, w[2]), (RESULT_CANCELED, 0));
    // A native that faults stops the answer.
    let k = e.vm.class_named("com/apportable/activity/VerdeActivity").unwrap();
    e.vm.register_native(k, "nativeOnActivityResult", "(IILandroid/content/Intent;)V", 0x6000_0000);
    assert!(vm.activity_result(&mut e.cpu, 9001, RESULT_CANCELED, None).is_err());
}

#[test]
fn only_picture_picks_want_a_picture() {
    let r = |action: &str, data: Option<&str>, mime: Option<&str>| ActivityRequest {
        code: 1,
        action: action.into(),
        data: data.map(Into::into),
        mime: mime.map(Into::into),
    };
    assert!(r("android.intent.action.GET_CONTENT", None, Some("image/*")).wants_image());
    assert!(!r("android.intent.action.GET_CONTENT", None, Some("text/plain")).wants_image());
    assert!(!r("android.intent.action.PICK", Some("content://contacts"), None).wants_image());
    assert!(!r("android.media.action.IMAGE_CAPTURE", None, Some("image/*")).wants_image());
}

fn write_pictures(dir: &std::path::Path) -> (String, String, String) {
    std::fs::create_dir_all(dir).unwrap();
    // 2x1 PNG: opaque red, half transparent white.
    let png = dir.join("pic.png");
    image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 255, 255, 255, 128]).unwrap().save(&png).unwrap();
    let jpg = dir.join("big.jpg");
    image::RgbImage::from_pixel(2048, 1024, image::Rgb([0, 0, 200])).save(&jpg).unwrap();
    let bad = dir.join("bad.png");
    std::fs::write(&bad, b"not a picture").unwrap();
    let s = |p: std::path::PathBuf| p.to_string_lossy().into_owned();
    (s(png), s(jpg), s(bad))
}

#[test]
fn picked_pictures_decode_into_premultiplied_bitmaps() {
    let mut e = Env::new();
    let dir = std::env::temp_dir().join(format!("bhpc-jni-pics-{}", std::process::id()));
    let (png, jpg, bad) = write_pictures(&dir);
    let get = |e: &mut Env, uri: &str| {
        let s = e.string(uri);
        let u = e.scall("android/net/Uri", "parse", "(Ljava/lang/String;)Landroid/net/Uri;", &[s]).unwrap();
        e.scall(
            "android/provider/MediaStore$Images$Media",
            "getBitmap",
            "(Landroid/content/ContentResolver;Landroid/net/Uri;)Landroid/graphics/Bitmap;",
            &[Value::Ref(None), u],
        )
        .unwrap()
        .as_ref()
    };
    let b = get(&mut e, &file_uri(&png)).expect("the PNG decodes");
    let (w, h, ptr) = crate::jlib::bitmap_info(&e.vm, b).unwrap();
    assert_eq!((w, h), (2, 1));
    assert_eq!(e.mem().read_bytes(ptr, 8).unwrap(), [255, 0, 0, 255, 128, 128, 128, 128]);
    let b = get(&mut e, &file_uri(&jpg)).expect("the JPEG decodes");
    let (w, h, ptr) = crate::jlib::bitmap_info(&e.vm, b).unwrap();
    assert_eq!((w, h), (1024, 512), "large pictures are scaled to 1024 pixels");
    let px = e.mem().read_bytes(ptr, 4).unwrap();
    assert!(px[2] > 150 && px[3] == 255, "blue, opaque: {px:?}");
    assert_eq!(get(&mut e, &file_uri(&bad)), None);
    assert_eq!(get(&mut e, "content://media/1"), None);
    assert!(e.log_text().contains("getBitmap: no picture"));
    let p = e.string(&png);
    let r =
        e.scall("android/graphics/BitmapFactory", "decodeFile", "(Ljava/lang/String;)Landroid/graphics/Bitmap;", &[p]);
    assert!(r.unwrap().as_ref().is_some());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn pictures_that_do_not_fit_the_guest_heap_give_no_bitmap() {
    let (mut e, fail) = Env::flaky();
    let dir = std::env::temp_dir().join(format!("bhpc-jni-pics-oom-{}", std::process::id()));
    let (png, _, _) = write_pictures(&dir);
    fail.store(true, std::sync::atomic::Ordering::Relaxed);
    let p = e.string(&png);
    let r =
        e.scall("android/graphics/BitmapFactory", "decodeFile", "(Ljava/lang/String;)Landroid/graphics/Bitmap;", &[p]);
    assert_eq!(r.unwrap(), Value::Ref(None));
    let _ = std::fs::remove_dir_all(dir);
}
