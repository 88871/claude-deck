mod core;
mod pty;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::path::PathBuf;
use core::session::{SessionManager, SessionState};
use pty::{resolve_claude_path, spawn_claude, PtyHandle};
use portable_pty::PtySize;
use std::io::Write;
use tauri::{AppHandle, Emitter, State};

struct AppState {
    manager: Mutex<SessionManager>,
    // Each PTY behind its own lock so a blocked/full session's write never
    // stalls another session's input (Review fix #7). The map lock is held
    // only long enough to clone out the Arc.
    ptys: Mutex<HashMap<String, Arc<Mutex<PtyHandle>>>>,
    // Resolved once at startup (Review nuance #3); None → claude not installed.
    claude_path: Option<String>,
}

fn emit_state(app: &AppHandle, id: &str, state: SessionState) {
    let _ = app.emit("session://state", serde_json::json!({ "id": id, "state": state }));
}

#[tauri::command]
fn start_session(app: AppHandle, state: State<AppState>, cwd: String) -> Result<String, String> {
    let claude_path = state
        .claude_path
        .clone()
        .ok_or("claude not found — install and log in to Claude Code, then restart")?;
    let path = PathBuf::from(&cwd);
    let id = { state.manager.lock().unwrap().create(path.clone()) };
    let handle = spawn_claude(app.clone(), id.clone(), &path, &claude_path)?;
    state.ptys.lock().unwrap().insert(id.clone(), Arc::new(Mutex::new(handle)));
    state.manager.lock().unwrap().set_state(&id, SessionState::Running);
    emit_state(&app, &id, SessionState::Running);
    Ok(id)
}

#[tauri::command]
fn write_to_pty(state: State<AppState>, id: String, data: String) -> Result<(), String> {
    let handle = {
        let ptys = state.ptys.lock().unwrap();
        ptys.get(&id).cloned().ok_or("unknown session")?
    };
    let mut h = handle.lock().unwrap();
    h.writer.write_all(data.as_bytes()).map_err(|e| e.to_string())?;
    h.writer.flush().map_err(|e| e.to_string())
}

#[tauri::command]
fn resize_pty(state: State<AppState>, id: String, cols: u16, rows: u16) -> Result<(), String> {
    let handle = {
        let ptys = state.ptys.lock().unwrap();
        ptys.get(&id).cloned().ok_or("unknown session")?
    };
    let h = handle.lock().unwrap();
    h.master
        .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_state = AppState {
        manager: Mutex::new(SessionManager::new()),
        ptys: Mutex::new(HashMap::new()),
        claude_path: resolve_claude_path(), // once, at startup (Review nuance #3)
    };
    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![start_session, write_to_pty, resize_pty])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
