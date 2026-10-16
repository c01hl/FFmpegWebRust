//! 设置接口（`docs/api-contract.md` §1.2）。

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use crate::api::error::{ApiError, IntoApiResult};
use crate::models::AppSettings;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(get_settings).put(put_settings))
        .route("/reset", post(reset_settings))
        .route("/port", get(port_info))
}

/// `GET /api/settings`
async fn get_settings(State(state): State<AppState>) -> Json<AppSettings> {
    Json(state.settings.get().await)
}

/// `PUT /api/settings` —— 整表替换（与原实现一致，不支持部分更新），返回被服务端修正的地方
async fn put_settings(
    State(state): State<AppState>,
    Json(incoming): Json<AppSettings>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let previous = state.settings.get().await;
    let saved = state.settings.save(incoming).await.api("保存设置失败")?;
    let warnings = corrections(&previous, &saved);

    if !warnings.is_empty() {
        tracing::info!("设置已保存，服务端修正了 {} 处", warnings.len());
    }

    Ok(Json(json!({ "settings": saved, "warnings": warnings })))
}

/// `POST /api/settings/reset`
async fn reset_settings(State(state): State<AppState>) -> Result<Json<AppSettings>, ApiError> {
    state.settings.reset().await.map(Json).api("恢复默认设置失败")
}

/// `GET /api/settings/port` —— 端口改动需要重启才生效，界面据此提示
async fn port_info(State(state): State<AppState>) -> Json<serde_json::Value> {
    let saved = state.settings.cached().await.listen_port;
    let effective = i32::from(state.listen.port);
    Json(json!({
        "savedListenPort": saved,
        "effectiveListenPort": effective,
        "restartRequired": saved != effective,
    }))
}

/// 对比「用户提交的」与「规范化之后的」设置，列出被服务端修正的字段。
///
/// 这是把原实现里「静默 normalize」变成可见反馈的地方 —— 用户改了并发数却发现没生效时，
/// 至少能从提示里知道原因。
fn corrections(before: &AppSettings, after: &AppSettings) -> Vec<String> {
    let _ = before;
    let mut warnings = Vec::new();

    if after.max_concurrent_tasks != before.max_concurrent_tasks
        && !(1..=16).contains(&before.max_concurrent_tasks)
    {
        warnings.push(format!(
            "并发任务数 {} 超出允许范围（1–16），已改为 {}。",
            before.max_concurrent_tasks, after.max_concurrent_tasks
        ));
    }
    if after.max_log_lines != before.max_log_lines {
        warnings.push(format!(
            "单任务日志行数 {} 超出允许范围（50–20000），已改为 {}。",
            before.max_log_lines, after.max_log_lines
        ));
    }
    if after.task_history_retention_days != before.task_history_retention_days {
        warnings.push(format!(
            "历史保留天数 {} 超出允许范围（0–3650），已改为 {}。",
            before.task_history_retention_days, after.task_history_retention_days
        ));
    }
    if after.listen_port != before.listen_port {
        warnings.push(format!(
            "监听端口 {} 不合法，已改为 {}。",
            before.listen_port, after.listen_port
        ));
    }
    if after.theme != before.theme {
        warnings.push(format!("主题「{}」无法识别，已改为跟随系统。", before.theme));
    }
    if after.output_suffix != before.output_suffix.trim() {
        warnings.push(format!("输出后缀已规范化为「{}」。", after.output_suffix));
    }
    if after.ffmpeg_path != before.ffmpeg_path.trim().trim_matches('"') {
        warnings.push("FFmpeg 路径已去除首尾空白与引号。".to_string());
    }

    warnings
}
