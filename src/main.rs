#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use dump_beacon::SanitizerResult;
use std::sync::Mutex;
use tauri::{Emitter, State, Window};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct AppState {
    cancel_token: Mutex<Option<CancellationToken>>,
    custom_rules: Mutex<Option<dump_beacon::SanitizationRules>>,
}

#[tauri::command]
async fn run_sanitizer(
    window: Window,
    state: State<'_, AppState>,
    input_path: String,
    output_path: String,
) -> Result<SanitizerResult, String> {
    let cancel_token = CancellationToken::new();
    {
        let mut token_lock = state.cancel_token.lock().unwrap();
        *token_lock = Some(cancel_token.clone());
    }

    let (tx, mut rx) = mpsc::channel(100);

    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                dump_beacon::ProgressEvent::Percentage(p) => {
                    let _ = window.emit("progress-update", p);
                }
                dump_beacon::ProgressEvent::BatchStatus(msg) => {
                    let _ = window.emit("batch-status", msg);
                }
            }
        }
    });

    let custom_rules = {
        let rules_lock = state.custom_rules.lock().unwrap();
        rules_lock.clone()
    };

    let metadata_result = std::fs::metadata(&input_path);
    let result = match metadata_result {
        Ok(metadata) if metadata.is_dir() => {
            dump_beacon::process_directory(input_path, output_path, Some(tx), cancel_token, custom_rules).await
        }
        Ok(_) => {
            dump_beacon::sanitize_sql_file(input_path, output_path, Some(tx), cancel_token, custom_rules).await
        }
        Err(e) => {
            Err(format!("Invalid input path: {}", e))
        }
    };

    {
        let mut token_lock = state.cancel_token.lock().unwrap();
        *token_lock = None;
    }

    result
}

#[tauri::command]
async fn cancel_job(state: State<'_, AppState>) -> Result<(), String> {
    let mut token_lock = state.cancel_token.lock().unwrap();
    if let Some(token) = token_lock.take() {
        token.cancel();
        Ok(())
    } else {
        Err("No active job to cancel".to_string())
    }
}

#[tauri::command]
async fn open_file_dialog() -> Result<String, String> {
    let result = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .add_filter("SQL Dumps", &["sql"])
            .set_title("Select a SQL Database Dump to Sanitize")
            .pick_file()
    })
    .await
    .map_err(|e| e.to_string())?;

    match result {
        Some(path) => Ok(path.display().to_string()),
        None => Err("No file selected".to_string()),
    }
}

#[tauri::command]
async fn save_file_dialog() -> Result<String, String> {
    let result = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .add_filter("SQL Dumps", &["sql"])
            .set_title("Save Sanitized SQL Dump")
            .save_file()
    })
    .await
    .map_err(|e| e.to_string())?;

    match result {
        Some(path) => Ok(path.display().to_string()),
        None => Err("No file selected".to_string()),
    }
}

#[tauri::command]
async fn pick_folder_dialog() -> Result<String, String> {
    let result = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Select Output Directory")
            .pick_folder()
    })
    .await
    .map_err(|e| e.to_string())?;

    match result {
        Some(path) => Ok(path.display().to_string()),
        None => Err("No folder selected".to_string()),
    }
}

#[tauri::command]
async fn is_dir(path: String) -> Result<bool, String> {
    let metadata = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    Ok(metadata.is_dir())
}

#[tauri::command]
async fn export_rules(rules: dump_beacon::SanitizationRules) -> Result<(), String> {
    let path = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .add_filter("JSON", &["json"])
            .set_file_name("rules.json")
            .set_title("Export Sanitization Rules")
            .save_file()
    })
    .await
    .map_err(|e| e.to_string())?;

    if let Some(p) = path {
        let json = serde_json::to_string_pretty(&rules).map_err(|e| e.to_string())?;
        std::fs::write(p, json).map_err(|e| e.to_string())?;
        Ok(())
    } else {
        Err("Export cancelled".to_string())
    }
}

#[tauri::command]
async fn get_current_rules(state: State<'_, AppState>) -> Result<dump_beacon::SanitizationRules, String> {
    let rules = {
        let rules_lock = state.custom_rules.lock().unwrap();
        rules_lock.clone()
    };
    Ok(rules.unwrap_or_default())
}

#[tauri::command]
async fn import_rules(state: State<'_, AppState>) -> Result<(String, dump_beacon::SanitizationRules), String> {
    let path = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .add_filter("JSON", &["json"])
            .set_title("Import Sanitization Rules")
            .pick_file()
    })
    .await
    .map_err(|e| e.to_string())?;

    if let Some(p) = path {
        let filename = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "custom_rules.json".to_string());
        let rules = dump_beacon::load_rules_from_file(p)?;
        {
            let mut rules_lock = state.custom_rules.lock().unwrap();
            *rules_lock = Some(rules.clone());
        }
        Ok((filename, rules))
    } else {
        Err("Import cancelled".to_string())
    }
}

#[tauri::command]
async fn preview_masking(state: State<'_, AppState>, path: String) -> Result<Vec<(String, String)>, String> {
    let rules = {
        let rules_lock = state.custom_rules.lock().unwrap();
        rules_lock.clone()
    };
    dump_beacon::preview_sanitize(path, 100, rules).await
}


fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(AppState {
            cancel_token: Mutex::new(None),
            custom_rules: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            run_sanitizer,
            open_file_dialog,
            save_file_dialog,
            pick_folder_dialog,
            is_dir,
            cancel_job,
            export_rules,
            import_rules,
            get_current_rules,
            preview_masking
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
