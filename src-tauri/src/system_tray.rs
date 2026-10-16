//! Optional Linux tray. Window close and shutdown policy remain in lib.rs.
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager,
};
const TRAY_ID: &str = "sonante";
#[derive(Debug, PartialEq)]
enum Action {
    Show,
    Quit,
    Ignore,
}
fn action(id: &str) -> Action {
    match id {
        "sonante.show" => Action::Show,
        "sonante.quit" => Action::Quit,
        _ => Action::Ignore,
    }
}
fn dispatch(id: &str, mut show: impl FnMut(), mut request_exit: impl FnMut()) {
    match action(id) {
        Action::Show => show(),
        Action::Quit => request_exit(),
        Action::Ignore => {}
    }
}
fn labels(language: &str) -> (&'static str, &'static str) {
    if language.to_ascii_lowercase().starts_with("pt") {
        ("Mostrar Sonante", "Sair")
    } else {
        ("Show Sonante", "Quit")
    }
}
#[cfg(target_os = "linux")]
fn first_indicator(mut probe: impl FnMut(&[u8]) -> Option<bool>) -> bool {
    for name in [
        b"libayatana-appindicator3.so.1\0".as_slice(),
        b"libappindicator3.so.1\0".as_slice(),
    ] {
        if let Some(supported) = probe(name) {
            return supported;
        }
    }
    false
}
#[cfg(target_os = "linux")]
fn indicator_supported() -> bool {
    // libappindicator-sys loads these sonames dynamically and panics if neither
    // exists. Preflight avoids that path, including in panic=abort builds.
    first_indicator(|name| {
        // SAFETY: constant NUL-terminated names; handle closed after symbol checks.
        let handle =
            unsafe { libc::dlopen(name.as_ptr().cast(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
        if handle.is_null() {
            return None;
        }
        let supported = [
            b"app_indicator_new\0".as_slice(),
            b"app_indicator_set_status\0",
            b"app_indicator_set_menu\0",
            b"app_indicator_set_title\0",
            b"app_indicator_set_icon_full\0",
            b"app_indicator_set_icon_theme_path\0",
        ]
        .iter()
        .all(|symbol| !unsafe { libc::dlsym(handle, symbol.as_ptr().cast()) }.is_null());
        unsafe {
            libc::dlclose(handle);
        }
        // The native loader chooses the first library that opens; don't select a
        // later library if this first one has an incompatible ABI.
        Some(supported)
    })
}
#[cfg(not(target_os = "linux"))]
fn indicator_supported() -> bool {
    true
}

pub fn install(app: &AppHandle) -> Result<(), &'static str> {
    if app.tray_by_id(TRAY_ID).is_some() {
        return Ok(());
    }
    if !indicator_supported() {
        return Err("indicator unavailable");
    }
    let language = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
        .unwrap_or_default();
    let (show_label, quit_label) = labels(&language);
    let show = MenuItem::with_id(app, "sonante.show", show_label, true, None::<&str>)
        .map_err(|_| "menu unavailable")?;
    let quit = MenuItem::with_id(app, "sonante.quit", quit_label, true, None::<&str>)
        .map_err(|_| "menu unavailable")?;
    let menu = Menu::with_items(app, &[&show, &quit]).map_err(|_| "menu unavailable")?;
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or("icon unavailable")?;
    // Avoid the native crate's shared /tmp fallback for its temporary PNG.
    let icon_dir = app
        .path()
        .app_cache_dir()
        .map_err(|_| "icon cache unavailable")?
        .join("tray");
    let mut directory = std::fs::DirBuilder::new();
    directory.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    directory
        .create(&icon_dir)
        .map_err(|_| "icon cache unavailable")?;
    TrayIconBuilder::with_id(TRAY_ID)
        .temp_dir_path(icon_dir)
        .icon(icon)
        .tooltip("Sonante")
        .menu(&menu)
        .on_menu_event(|app, event| {
            dispatch(
                event.id.as_ref(),
                || {
                    if let Some(window) = app.get_webview_window("main") {
                        if window
                            .show()
                            .and_then(|_| window.unminimize())
                            .and_then(|_| window.set_focus())
                            .is_err()
                        {
                            eprintln!("[Tray] Main window could not be shown.");
                        }
                    }
                },
                || {
                    // Generates ExitRequested. Its existing transaction guard calls the
                    // sole coordinated shutdown worker; never exit the process directly.
                    app.exit(0);
                },
            )
        })
        .build(app)
        .map_err(|_| "tray unavailable")?;
    // Tauri's resource table retains the registered icon for the app lifetime.
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    #[test]
    fn tray_menu_restores_or_requests_exit_and_ignores_other_ids() {
        let calls = RefCell::new(vec![]);
        for id in ["sonante.show", "unknown", "sonante.quit"] {
            dispatch(
                id,
                || calls.borrow_mut().push("show"),
                || calls.borrow_mut().push("request_exit"),
            );
        }
        assert_eq!(*calls.borrow(), ["show", "request_exit"]);
        assert_eq!(action("quit;process.exit"), Action::Ignore);
    }
    #[test]
    fn tray_labels_follow_system_language_without_touching_audio() {
        assert_eq!(labels("pt_BR.UTF-8"), ("Mostrar Sonante", "Sair"));
        assert_eq!(labels("en_US.UTF-8"), ("Show Sonante", "Quit"));
        assert_eq!(labels("C"), ("Show Sonante", "Quit"));
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn tray_missing_or_incompatible_indicator_never_enters_native_builder() {
        assert!(!first_indicator(|_| None));
        let mut calls = 0;
        assert!(!first_indicator(|_| {
            calls += 1;
            Some(false)
        }));
        assert_eq!(
            calls, 1,
            "must follow the native loader's first library choice"
        );
        let mut calls = 0;
        assert!(first_indicator(|_| {
            calls += 1;
            if calls == 1 {
                None
            } else {
                Some(true)
            }
        }));
        assert_eq!(calls, 2);
    }
}
