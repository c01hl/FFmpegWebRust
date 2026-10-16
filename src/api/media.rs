//! 媒体与文件接口（`docs/api-contract.md` §1.4）。
//!
//! ⚠️ 原生文件选择器由**服务端**执行（osascript / zenity / kdialog / PowerShell）：
//! 浏览器拿不到绝对路径，所以这个接口隐含「服务端与用户在同一台机器」的前提，
//! 与默认只监听回环地址是一致的。

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::api::error::ApiError;
use crate::files::{self, dialog};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/media/info", post(media_info))
        .route("/media/scan", post(scan_media))
        .route("/files/extensions", get(file_extensions))
        .route("/files/dialog", post(open_dialog))
        .route("/files/dialog/backend", get(dialog_backend))
        .route("/files/check", post(check_path))
        .route("/files/reveal", post(reveal_path))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PathRequest {
    path: String,
}

/// `POST /api/media/info`
async fn media_info(
    State(state): State<AppState>,
    Json(request): Json<PathRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if request.path.trim().is_empty() {
        return Err(ApiError::bad_request("PATH_REQUIRED", "请先选择要转换的文件。"));
    }

    match state.ffmpeg.probe_media(&request.path).await {
        Ok(media) => {
            let info = crate::queue::preview::MediaInfoDto::from_media(&media);
            Ok(Json(json!({ "info": info, "reason": serde_json::Value::Null })))
        }
        Err(error) => {
            // 探测失败不是致命错误：界面仍然可以让用户继续，只是没有时长/分辨率信息
            tracing::warn!("ffprobe 探测失败 {}：{error}", request.path);
            Ok(Json(json!({
                "info": null,
                "reason": "无法读取该文件的媒体信息（文件可能损坏或不是媒体文件）。",
            })))
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScanRequest {
    directory: String,
    #[serde(default)]
    extensions: Vec<String>,
    #[serde(default)]
    recursive: bool,
}

/// `POST /api/media/scan`
async fn scan_media(
    State(_state): State<AppState>,
    Json(request): Json<ScanRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if request.directory.trim().is_empty() {
        return Err(ApiError::bad_request("DIRECTORY_REQUIRED", "请先选择要扫描的文件夹。"));
    }

    let entries = tokio::task::spawn_blocking({
        let directory = request.directory.clone();
        let extensions = request.extensions.clone();
        move || files::scan_media_files(&directory, &extensions, request.recursive)
    })
    .await
    .map_err(|error| ApiError::internal("扫描目录失败", error))?;

    Ok(Json(json!({ "files": entries })))
}

/// `GET /api/files/extensions`
async fn file_extensions() -> Json<serde_json::Value> {
    Json(json!({ "extensions": files::supported_media_extensions() }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DialogBody {
    kind: dialog::DialogKind,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    initial_directory: Option<String>,
    #[serde(default)]
    filter: Option<String>,
}

/// `POST /api/files/dialog`
async fn open_dialog(
    State(_state): State<AppState>,
    Json(body): Json<DialogBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut request = match body.kind {
        dialog::DialogKind::File => dialog::DialogRequest::file(),
        dialog::DialogKind::Folder => dialog::DialogRequest::folder(),
    };
    if let Some(title) = body.title.filter(|t| !t.trim().is_empty()) {
        request.title = title;
    }
    request.initial_directory = body.initial_directory;
    request.filter = body.filter;

    let result = dialog::pick(request).await;
    Ok(Json(json!(result)))
}

/// `GET /api/files/dialog/backend`
async fn dialog_backend() -> Json<dialog::DialogBackend> {
    Json(dialog::backend())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckBody {
    path: String,
    #[serde(default = "default_kind")]
    kind: String,
}

fn default_kind() -> String {
    "file".to_string()
}

/// `POST /api/files/check`
async fn check_path(Json(body): Json<CheckBody>) -> Result<Json<serde_json::Value>, ApiError> {
    let path = body.path.clone();
    let is_dir = body.kind.eq_ignore_ascii_case("dir") || body.kind.eq_ignore_ascii_case("folder");

    let result = tokio::task::spawn_blocking(move || {
        let absolute = crate::paths::to_absolute(&path);
        json!({
            "exists": absolute.exists(),
            "accessible": if is_dir { absolute.is_dir() } else { files::is_file_accessible(&path) },
            "writable": if is_dir { files::is_directory_writable(&path) } else { crate::files::output_path::is_output_path_writable(&path) },
            "size": files::get_file_size(&path),
            "diskFree": files::available_disk_space(&path),
        })
    })
    .await
    .map_err(|error| ApiError::internal("检查路径失败", error))?;

    Ok(Json(result))
}

/// `POST /api/files/reveal`
async fn reveal_path(Json(body): Json<PathRequest>) -> Result<Json<serde_json::Value>, ApiError> {
    let path = body.path.clone();
    let outcome = tokio::task::spawn_blocking(move || files::reveal_in_file_manager(&path))
        .await
        .map_err(|error| ApiError::internal("打开文件管理器失败", error))?;

    match outcome {
        Ok(()) => Ok(Json(json!({ "ok": true, "reason": null }))),
        Err(reason) => Ok(Json(json!({ "ok": false, "reason": reason }))),
    }
}
