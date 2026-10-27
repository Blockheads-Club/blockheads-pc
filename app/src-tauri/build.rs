//! The app commands go through the access control list like the plugins' commands: only the windows whose
//! capability grants them (main, splash; `permissions/app-commands.toml`) can call them. The window that shows the
//! game's pages (server HTML, scripts allowed) has no capability, so it cannot reach any of them.

/// Every command of `generate_handler!` in `src/lib.rs` (kept in step by a test there).
const COMMANDS: &[&str] = &[
    "app_info",
    "splash_set",
    "splash_get",
    "splash_retry",
    "app_quit",
    "tray_labels",
    "main_ready",
    "boot",
    "save_config",
    "save_prefs",
    "finish_setup",
    "check_apk",
    "use_apk",
    "normalize_binding",
    "display_info",
    "window_resize",
    "import_saves",
    "data_size",
    "move_data_dir",
    "open_path",
    "game_start",
    "game_stop",
    "game_poll",
    "embed_layout",
    "embed_reveal",
    "embed_focus",
    "embed_fullscreen",
    "save_log",
    "update_check",
    "update_install",
    "update_progress",
    "update_restart",
    "mods_overview",
    "mods_index",
    "mods_plan",
    "mods_inspect_file",
    "mods_inspect_link",
    "mods_install",
    "mods_set_enabled",
    "mods_set_grant",
    "mods_remove",
    "mods_auto_update",
    "mods_servers",
    "mods_clear_server",
    "mods_clear_servers",
    "mods_dev_inspect",
    "mods_dev_reload",
    "mods_dev_fingerprint",
    "mods_dev_join",
];

fn main() {
    let manifest = tauri_build::AppManifest::new().commands(COMMANDS);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest)).expect("failed to run tauri-build");
}
