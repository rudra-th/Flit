//! The Flit desktop shell.
//!
//! Three things live here, and nothing else:
//!
//! * one engine, started in `setup` and shared by every command;
//! * a tray icon that keeps the receiver alive when the window is closed;
//! * the commands in [`commands`], which are the only door the UI has.
//!
//! Protocol logic stays in `flit-core`. A shell that starts making protocol
//! decisions is a shell that will disagree with the other ones.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod dto;
mod events;
mod state;
mod tray;

use tauri::{AppHandle, Manager, WindowEvent};
use tauri_plugin_dialog::DialogExt;

use crate::state::Shell;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(tauri::generate_handler![
            commands::status,
            commands::peers,
            commands::history,
            commands::doctor,
            commands::inspect_paths,
            commands::send_to_peer,
            commands::send_text_to_peer,
            commands::send_to_address,
            commands::cancel_transfer,
            commands::read_clipboard,
        ])
        .setup(|app| {
            match Shell::start(app.handle()) {
                Ok(shell) => {
                    app.manage(shell);
                }
                Err(err) => {
                    // Starting without an engine would leave a window whose
                    // every button fails, so say what happened and leave.
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.hide();
                    }
                    fail(app.handle(), &err);
                }
            }
            tray::install(app)?;
            Ok(())
        })
        // The window has no frame, so its close button is ours: it hides the
        // window and leaves the receiver listening. Quitting is the tray's job.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("Flit: the window could not be built")
        // No shutdown hook: ending the process closes the sockets and the
        // receiver sweeps a session it hears nothing more about. Stopping the
        // engine first would keep the tray icon on screen while it waits.
        .run(|_app, _event| {});
}

/// Report a startup failure where a person will see it.
///
/// The dialog is built on its own thread because a blocking dialog on the main
/// thread would stop the event loop it is waiting on.
fn fail(app: &AppHandle, message: &str) {
    let app = app.clone();
    let text = format!("Flit could not start.\n\n{message}");
    log::error!("{text}");
    std::thread::spawn(move || {
        app.dialog()
            .message(text)
            .title("Flit could not start")
            .blocking_show();
        std::process::exit(1);
    });
}
