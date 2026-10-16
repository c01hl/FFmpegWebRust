//! 系统与环境相关接口（`docs/api-contract.md` §1.1）。

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::api::error::{ApiError, IntoApiResult};
use crate::paths;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/status", get(status))
        .route("/ffmpeg", get(ffmpeg_info))
        .route("/ffmpeg/validate", post(validate_ffmpeg))
        .route("/encoders", get(encoders))
        .route("/encoders/recommend", get(recommend_encoder))
        .route("/paths", get(system_paths))
        .route("/queue", get(queue_state))
}

#[derive(Debug, Deserialize)]
struct ForceQuery {
    #[serde(default)]
    force: bool,
}

/// `GET /api/system/status`
async fn status(
    State(state): State<AppState>,
    Query(query): Query<ForceQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let info = state.ffmpeg.info(query.force).await;
    let report = state.hardware.detect(query.force).await;
    let templates = state.templates.list(true).await.api("读取模板失败")?;
    let task_count = state.store.count_tasks().await.api("统计任务失败")?;

    let available_hardware = report
        .encoders
        .iter()
        .filter(|e| e.available && e.is_hardware)
        .count();

    let mut notices = Vec::new();
    if info.as_ref().map(|i| i.available) != Some(true) {
        notices.push(state.ffmpeg.paths().hint.clone());
    }
    if !report.has_hardware_acceleration() {
        notices.push("本机没有检测到可用的硬件编码器，将使用软件编码（速度较慢，画质相同）。".to_string());
    }
    if !crate::web_assets::frontend_available() {
        notices.push("内嵌的前端产物为空，请执行 `cd web && pnpm build` 后重新编译。".to_string());
    }

    let data_directory = paths::data_directory();
    let data_directory_writable = crate::files::is_directory_writable(&data_directory.to_string_lossy());

    Ok(Json(json!({
        "version": crate::paths::APP_VERSION,
        "ffmpegAvailable": info.as_ref().map(|i| i.available).unwrap_or(false),
        "ffmpegVersion": info.as_ref().map(|i| i.version.clone()).unwrap_or_default(),
        "ffmpegPath": info.as_ref().map(|i| i.path.clone()).unwrap_or_default(),
        "ffprobePath": state.ffmpeg.paths().ffprobe,
        "platform": paths::platform_display_name(),
        "dataDirectory": data_directory.to_string_lossy(),
        "dataDirectoryWritable": data_directory_writable,
        "defaultOutputDirectory": paths::default_output_directory().to_string_lossy(),
        "databasePath": paths::database_path().to_string_lossy(),
        "availableHardwareEncoderCount": available_hardware,
        "templateCount": templates.len(),
        "taskCount": task_count,
        "hwaccels": report.hwaccels,
        "notices": notices,
        "lastHardwareDetection": report.probed_at,
    })))
}

/// `GET /api/system/ffmpeg`
async fn ffmpeg_info(
    State(state): State<AppState>,
    Query(query): Query<ForceQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let paths = state.ffmpeg.paths();
    let info = state.ffmpeg.info(query.force).await;

    Ok(Json(json!({
        "info": info,
        "paths": {
            "ffmpeg": paths.ffmpeg,
            "ffmpegResolved": paths.ffmpeg_resolved,
            "ffprobe": paths.ffprobe,
            "ffprobeResolved": paths.ffprobe_resolved,
            "source": paths.source,
            "hint": paths.hint,
        }
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ValidateFfmpegRequest {
    path: String,
}

/// `POST /api/system/ffmpeg/validate`
async fn validate_ffmpeg(
    State(state): State<AppState>,
    Json(request): Json<ValidateFfmpegRequest>,
) -> Json<serde_json::Value> {
    let (valid, version) = state.settings.validate_ffmpeg_path(&request.path).await;
    Json(json!({ "valid": valid, "version": version }))
}

/// `GET /api/system/encoders`
async fn encoders(
    State(state): State<AppState>,
    Query(query): Query<ForceQuery>,
) -> Json<serde_json::Value> {
    let report = state.hardware.detect(query.force).await;
    // 检测结果顺手落库，供设置页在检测尚未跑完时展示上次结果
    if let Err(error) = state.store.upsert_encoders(&to_cache_rows(&report)).await {
        tracing::warn!("写入硬件编码器缓存失败：{error}");
    }
    Json(crate::api::dto::hardware_report_json(&report))
}

/// 把检测报告投影成可持久化的缓存行。
///
/// `id` 只在**首次插入**时使用（`upsert_encoders` 按 `name` 冲突更新并保留原 id），
/// 所以这里每行现生成一个即可。
fn to_cache_rows(report: &crate::ffmpeg::hardware::HardwareReport) -> Vec<crate::models::HardwareEncoder> {
    report
        .encoders
        .iter()
        .map(|item| crate::models::HardwareEncoder {
            id: crate::models::new_id(),
            name: item.encoder_id.clone(),
            display_name: item.display_name.clone(),
            encoder_type: item.encoder_type,
            family: item.family.clone(),
            codec: item.target_codec.clone(),
            is_available: item.available,
            supports_quality_options: item.verified_by_trial,
            last_checked_at: item.last_checked_at,
            unavailable_reason: if item.reason.trim().is_empty() { None } else { Some(item.reason.clone()) },
            missing_from_build: item.missing_from_build,
            ffmpeg_version: report.ffmpeg_version.clone(),
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct RecommendQuery {
    codec: String,
    #[serde(default)]
    required: Option<String>,
}

/// `GET /api/system/encoders/recommend?codec=h264&required=nvenc`
async fn recommend_encoder(
    State(state): State<AppState>,
    Query(query): Query<RecommendQuery>,
) -> Json<serde_json::Value> {
    match state.hardware.recommended_encoder(&query.codec, query.required.as_deref()).await {
        Some(encoder) => Json(json!({ "encoder": encoder })),
        None => {
            let reason = match query.required.as_deref() {
                Some(required) => crate::queue::preview::strict_hardware_error(Some(required)),
                None => format!("本机没有可用于 {} 编码的编码器。", query.codec),
            };
            Json(json!({ "encoder": null, "reason": reason }))
        }
    }
}

/// `GET /api/system/paths`
async fn system_paths() -> Json<serde_json::Value> {
    let data_directory = paths::data_directory();
    Json(json!({
        "dataDirectory": data_directory.to_string_lossy(),
        "defaultOutputDirectory": paths::default_output_directory().to_string_lossy(),
        "databasePath": paths::database_path().to_string_lossy(),
        "platform": paths::platform_display_name(),
        "isContainer": paths::is_container(),
        "dataDirectoryWritable": crate::files::is_directory_writable(&data_directory.to_string_lossy()),
    }))
}

/// `GET /api/system/queue`
async fn queue_state(State(state): State<AppState>) -> Json<serde_json::Value> {
    let max_concurrent = state.settings.cached().await.max_concurrent_tasks;
    Json(json!({
        "queued": state.queue.queued_count(),
        "active": state.queue.active_count(),
        "maxConcurrentTasks": max_concurrent,
    }))
}
