//! HTTP 接口层（axum）。
//!
//! 契约见 `docs/api-contract.md`：REST 前缀 `/api`，JSON 键名 camelCase，枚举用字符串，
//! 错误统一为 `{"error":{"code","message"}}`，其余路径回落到内嵌前端（SPA）。
//!
//! ⚠️ 本文件目前只有骨架（健康检查 + 静态资源 + 错误类型），
//! 各资源路由由实现方补齐。

pub mod dto;
pub mod error;
pub mod media;
pub mod settings;
pub mod system;
pub mod tasks;
pub mod templates;
pub mod ws;

use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use crate::state::AppState;

pub use error::ApiError;

/// 组装全部路由。
///
/// 分组的路径前缀在这里统一加上：各子模块内部的路径都是**相对自己分组**的
/// （例如 `settings.rs` 里写 `/reset`，最终对外是 `/api/settings/reset`），
/// 这样可以避免不同分组都去抢 `/` 这个路径。
pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .nest("/system", system::routes())
        .nest("/settings", settings::routes())
        .nest("/templates", templates::routes())
        .merge(media::routes())
        .merge(tasks::routes())
        .merge(ws::routes())
        .with_state(state);

    Router::new()
        .nest("/api", api)
        // 内嵌前端产物 + SPA 回落（必须放在最后）
        .fallback(crate::web_assets::static_handler)
}

/// `GET /api/health`：版本、平台与 FFmpeg 解析结果，便于排查
async fn health(axum::extract::State(state): axum::extract::State<AppState>) -> Json<serde_json::Value> {
    let paths = state.ffmpeg.paths();

    Json(json!({
        "status": "ok",
        "version": crate::paths::APP_VERSION,
        "platform": crate::paths::platform_display_name(),
        "ffmpeg": paths.ffmpeg,
        "ffmpegResolved": paths.ffmpeg_resolved,
        "ffprobe": paths.ffprobe,
        "ffprobeResolved": paths.ffprobe_resolved,
    }))
}
