//! The tray icon, which is what keeps Flit reachable after the window closes.
//!
//! Closing the window hides it; the receiver keeps listening. Quitting is a
//! deliberate choice here, because a transfer app that stops receiving the
//! moment its window is dismissed is not doing its job.

use tauri::{
    App, AppHandle, Manager,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

/// Add the tray icon and its two commands.
pub fn install(app: &mut App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Flit", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Flit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    let mut builder = TrayIconBuilder::with_id("flit")
        .tooltip("Flit — ready to receive")
        .menu(&menu)
        // The menu belongs to the right button; a left click is "show me".
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => reveal(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                reveal(tray.app_handle());
            }
        });

    // Without the generated icon the tray would have nothing to draw, so the
    // window icon is the fallback rather than a broken entry in the corner.
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)?;
    Ok(())
}

/// Bring the window back from the tray.
fn reveal(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
