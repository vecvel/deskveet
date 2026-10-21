//! The only surface the UI can call. Every command re-validates its inputs.

use crate::platform::Platform;
use crate::sysmon::{Monitor, Overview, ProcList, ProcQuery};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::State;

pub struct AppState {
    pub mon: Mutex<Monitor>,
}

impl AppState {
    pub fn new() -> Self {
        Self { mon: Mutex::new(Monitor::new()) }
    }
}

// `async` makes Tauri run these on its thread pool instead of the main (UI) thread.
#[tauri::command(async)]
pub fn overview(state: State<'_, AppState>) -> Overview {
    state.mon.lock().unwrap().overview()
}

#[tauri::command(async)]
pub fn processes(state: State<'_, AppState>, query: ProcQuery) -> ProcList {
    state.mon.lock().unwrap().processes(&query)
}

#[tauri::command(async)]
pub fn quit_process(state: State<'_, AppState>, pid: u32, start: u64, force: bool) -> Result<(), String> {
    state.mon.lock().unwrap().signal(pid, start, force)
}

fn existing(path: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(path);
    if !p.is_absolute() || !p.exists() {
        return Err("That path no longer exists.".into());
    }
    Ok(p)
}

#[tauri::command(async)]
pub fn reveal(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let p = existing(&path)?;
    state.mon.lock().unwrap().plat().reveal(&p).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn open_path(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let p = existing(&path)?;
    state.mon.lock().unwrap().plat().open(&p).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn quick_look(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let p = existing(&path)?;
    state.mon.lock().unwrap().plat().quick_look(&p).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn open_fda_settings(state: State<'_, AppState>) -> Result<(), String> {
    state.mon.lock().unwrap().plat().open_full_disk_access_settings().map_err(|e| e.to_string())
}
