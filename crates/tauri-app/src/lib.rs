//! Tauri 外壳:把 `zip-tool-core` 的能力通过 `#[tauri::command]` 暴露给前端。
//! 命令在 `spawn_blocking` 中调用 core,进度经 `tauri::ipc::Channel` 推给前端;
//! UI 的"取消"按钮经 `CancelFlag` 在条目间生效。

mod job;

use std::path::Path;
use std::sync::Mutex;

use serde::Serialize;
use tauri::State;
use tauri::ipc::Channel;

use job::CancelFlag;
use zip_tool_core::Progress;

/// 全局状态:当前任务(压缩)的取消旗标
#[derive(Default)]
struct AppState {
    current: Mutex<Option<CancelFlag>>,
}

#[derive(Serialize, Clone)]
struct ProgressEvent {
    done: usize,
    total: usize,
    entry: String,
}

impl From<&Progress> for ProgressEvent {
    fn from(p: &Progress) -> Self {
        Self {
            done: p.done,
            total: p.total,
            entry: p.entry.clone(),
        }
    }
}

#[derive(Serialize)]
struct ArchiveInfo {
    archive: String,
    original: String,
}

#[derive(Serialize)]
struct CompressResult {
    cancelled: bool,
    entries: Vec<ArchiveInfo>,
}

#[tauri::command]
fn ping() -> String {
    "pong".into()
}

/// 批量压缩:input 下每个顶层条目各压缩为一个包。
/// split_size 传 "2g"/"500m"/"65536",null 表示不分包。
#[tauri::command]
async fn compress(
    input: String,
    output: String,
    split_size: Option<String>,
    password: Option<String>,
    random_name: bool,
    on_progress: Channel<ProgressEvent>,
    state: State<'_, AppState>,
) -> Result<CompressResult, String> {
    let split_size = match split_size.as_deref().map(zip_tool_core::parse_size) {
        Some(Ok(v)) => Some(v),
        Some(Err(e)) => return Err(e),
        None => None,
    };

    let flag = CancelFlag::new();
    *state
        .current
        .lock()
        .map_err(|_| "state poisoned".to_string())? = Some(flag.clone());

    let result = tauri::async_runtime::spawn_blocking(move || {
        let opts = zip_tool_core::Options {
            split_size,
            password,
            random_name,
        };
        zip_tool_core::compress_dir(
            Path::new(&input),
            Path::new(&output),
            &opts,
            &flag.into_callback(|p| {
                let _ = on_progress.send(ProgressEvent::from(p));
            }),
        )
    })
    .await
    .map_err(|e| e.to_string())?;

    let report = result.map_err(|e| e.to_string())?;
    Ok(CompressResult {
        cancelled: report.cancelled,
        entries: report
            .entries
            .into_iter()
            .map(|e| ArchiveInfo {
                archive: e.archive.display().to_string(),
                original: e.original,
            })
            .collect(),
    })
}

/// 解压 .zip 或第一个分卷(.zip.001)
#[tauri::command]
async fn extract_archive(
    input: String,
    output: String,
    password: Option<String>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        zip_tool_core::extract(Path::new(&input), Path::new(&output), password.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

/// 取消当前压缩任务(在当前条目完成后生效)
#[tauri::command]
fn cancel_current(state: State<'_, AppState>) {
    if let Ok(current) = state.current.lock() {
        if let Some(flag) = current.as_ref() {
            flag.cancel();
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            ping,
            compress,
            extract_archive,
            cancel_current
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
