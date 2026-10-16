//! 任务、批次与历史接口（`docs/api-contract.md` §1.5–§1.7）。

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::api::error::{ApiError, IntoApiResult};
use crate::models::enums::TaskStatus;
use crate::queue::preview::{PreviewRequest, PreviewResponse};
use crate::queue::task_service::{CreateBatchRequest, CreateTaskRequest, DeleteTasksResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/tasks", get(list_tasks).post(create_task).delete(delete_tasks_by_status))
        .route("/tasks/preview", post(preview))
        .route("/tasks/running", get(running_tasks))
        .route("/tasks/pending", get(pending_tasks))
        .route("/tasks/delete", post(delete_tasks))
        .route("/tasks/{id}", get(get_task))
        .route("/tasks/{id}/start", post(start_task))
        .route("/tasks/{id}/cancel", post(cancel_task))
        .route("/tasks/{id}/retry", post(retry_task))
        .route("/tasks/{id}/log", get(task_log))
        .route("/batches", get(list_batches).post(create_batch))
        .route("/batches/{id}", get(get_batch).delete(delete_batch))
        .route("/batches/{id}/tasks", get(batch_tasks))
        .route("/batches/{id}/cancel", post(cancel_batch))
        .route("/batches/{id}/retry-failed", post(retry_failed))
        .route("/history/cleanup", post(cleanup_history))
        .route("/history/clear", post(clear_history))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskListQuery {
    #[serde(default)]
    status: Option<TaskStatus>,
    #[serde(default)]
    batch_id: Option<String>,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_limit() -> i64 {
    50
}

/// `GET /api/tasks`
async fn list_tasks(
    State(state): State<AppState>,
    Query(query): Query<TaskListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if let Some(batch_id) = query.batch_id.as_deref().filter(|id| !id.is_empty()) {
        let items = state.tasks.list_tasks_by_batch(batch_id).await.api("读取任务失败")?;
        let total = items.len() as i64;
        return Ok(Json(json!({
            "items": crate::api::dto::tasks_json(&items),
            "total": total,
            "limit": query.limit,
            "offset": query.offset,
        })));
    }

    let (items, total) = state
        .tasks
        .list_tasks(query.limit, query.offset, query.status)
        .await
        .api("读取任务失败")?;

    Ok(Json(json!({
        "items": crate::api::dto::tasks_json(&items),
        "total": total,
        "limit": query.limit,
        "offset": query.offset,
    })))
}

/// `POST /api/tasks` —— 创建任务（可选立即开始）
async fn create_task(
    State(state): State<AppState>,
    Json(request): Json<CreateTaskRequest>,
) -> Result<(axum::http::StatusCode, Json<serde_json::Value>), ApiError> {
    let response = state
        .tasks
        .create_task(request)
        .await
        .map_err(|error| {
            let message = error.to_string();
            if message.contains("模板") && message.contains("不存在") {
                ApiError::not_found("TEMPLATE_NOT_FOUND", message)
            } else {
                ApiError::bad_request("TASK_INVALID", message)
            }
        })?;

    Ok((
        axum::http::StatusCode::CREATED,
        Json(json!({ "task": crate::api::dto::task_json(&response.task), "enqueued": response.enqueued })),
    ))
}

/// `POST /api/tasks/preview` —— 命令预览 + 输出路径冲突解析（界面不做任何本地推断）
async fn preview(
    State(state): State<AppState>,
    Json(request): Json<PreviewRequest>,
) -> Result<Json<PreviewResponse>, ApiError> {
    state
        .tasks
        .preview(request)
        .await
        .map(Json)
        .map_err(|error| preview_error(&error.to_string()))
}

fn preview_error(message: &str) -> ApiError {
    if message.contains("模板") && message.contains("不存在") {
        ApiError::not_found("TEMPLATE_NOT_FOUND", message.to_string())
    } else if message.contains("请先选择") || message.contains("不能为空") {
        ApiError::bad_request("PREVIEW_INVALID", message.to_string())
    } else {
        ApiError::internal("生成命令预览失败", message)
    }
}

/// `GET /api/tasks/running`
async fn running_tasks(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let items = state.tasks.running_tasks().await.api("读取运行中任务失败")?;
    Ok(Json(json!(crate::api::dto::tasks_json(&items))))
}

/// `GET /api/tasks/pending`
async fn pending_tasks(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let items = state.tasks.pending_tasks().await.api("读取等待中任务失败")?;
    Ok(Json(json!(crate::api::dto::tasks_json(&items))))
}

/// `GET /api/tasks/{id}`
async fn get_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let task = state
        .tasks
        .get_task(&id)
        .await
        .api("读取任务失败")?
        .ok_or_else(|| ApiError::not_found("TASK_NOT_FOUND", "任务不存在或已被删除。"))?;
    Ok(Json(crate::api::dto::task_json(&task)))
}

/// `POST /api/tasks/{id}/start`
async fn start_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    match state.tasks.start_task(&id).await {
        // 契约：返回更新后的 TaskDto 本身（前端不加包装直接读）
        Ok(task) => Ok(Json(crate::api::dto::task_json(&task))),
        Err(error) => {
            let message = error.to_string();
            if message.contains("不存在") {
                Err(ApiError::not_found("TASK_NOT_FOUND", message))
            } else {
                // 已终态等可预期冲突
                Err(ApiError::conflict("TASK_NOT_PENDING", message))
            }
        }
    }
}

#[derive(Debug, Deserialize, Default)]
struct CancelBody {
    #[serde(default)]
    graceful: bool,
}

/// `POST /api/tasks/{id}/cancel`
async fn cancel_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<CancelBody>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let graceful = body.map(|Json(body)| body.graceful).unwrap_or(false);
    state.tasks.cancel_task(&id, graceful).await.api("取消任务失败")?;

    // 契约：返回取消后的 TaskDto（界面据此立刻刷新该行，而不是等 WebSocket 事件）
    let task = state
        .tasks
        .get_task(&id)
        .await
        .api("读取任务失败")?
        .ok_or_else(|| ApiError::not_found("TASK_NOT_FOUND", "任务不存在或已被删除。"))?;
    Ok(Json(crate::api::dto::task_json(&task)))
}

/// `POST /api/tasks/{id}/retry`
async fn retry_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let task = state
        .tasks
        .retry_task(&id)
        .await
        .api("重试任务失败")?
        .ok_or_else(|| ApiError::not_found("TASK_NOT_FOUND", "任务不存在或已被删除。"))?;

    Ok(Json(json!({ "task": crate::api::dto::task_json(&task) })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogQuery {
    #[serde(default)]
    tail_lines: Option<usize>,
}

/// `GET /api/tasks/{id}/log?tailLines=200`
async fn task_log(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<LogQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let log = state
        .tasks
        .task_log(&id, query.tail_lines)
        .await
        .api("读取任务日志失败")?
        .ok_or_else(|| ApiError::not_found("TASK_NOT_FOUND", "任务不存在或已被删除。"))?;

    let max_log_lines = state.settings.cached().await.max_log_lines;
    Ok(Json(json!({ "taskId": id, "logOutput": log, "maxLogLines": max_log_lines })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteTasksBody {
    ids: Vec<String>,
}

/// `POST /api/tasks/delete`
async fn delete_tasks(
    State(state): State<AppState>,
    Json(body): Json<DeleteTasksBody>,
) -> Result<Json<DeleteTasksResult>, ApiError> {
    state.tasks.delete_tasks(&body.ids).await.map(Json).api("删除任务失败")
}

#[derive(Debug, Deserialize)]
struct DeleteByStatusQuery {
    status: TaskStatus,
}

/// `DELETE /api/tasks?status=Failed`
async fn delete_tasks_by_status(
    State(state): State<AppState>,
    Query(query): Query<DeleteByStatusQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let deleted = state
        .tasks
        .delete_tasks_by_status(query.status)
        .await
        .api("删除任务失败")?;
    Ok(Json(json!({ "deleted": deleted })))
}

// ────────────────────────────────────────────────────────────────────────────
// 批次
// ────────────────────────────────────────────────────────────────────────────

/// `POST /api/batches`
async fn create_batch(
    State(state): State<AppState>,
    Json(request): Json<CreateBatchRequest>,
) -> Result<(axum::http::StatusCode, Json<serde_json::Value>), ApiError> {
    let response = state
        .tasks
        .create_batch(request)
        .await
        .map_err(|error| ApiError::bad_request("BATCH_INVALID", error.to_string()))?;

    Ok((
        axum::http::StatusCode::CREATED,
        Json(json!({
            "batch": crate::api::dto::batch_json(&response.batch),
            "tasks": crate::api::dto::tasks_json(&response.tasks),
        })),
    ))
}

#[derive(Debug, Deserialize)]
struct PagingQuery {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

/// `GET /api/batches`
async fn list_batches(
    State(state): State<AppState>,
    Query(query): Query<PagingQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (items, total) = state
        .tasks
        .list_batches(query.limit, query.offset)
        .await
        .api("读取批次失败")?;
    let items: Vec<serde_json::Value> = items.iter().map(crate::api::dto::batch_json).collect();
    Ok(Json(json!({
        "items": items,
        "total": total,
        "limit": query.limit,
        "offset": query.offset,
    })))
}

/// `GET /api/batches/{id}`
async fn get_batch(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let batch = state
        .tasks
        .get_batch(&id)
        .await
        .api("读取批次失败")?
        .ok_or_else(|| ApiError::not_found("BATCH_NOT_FOUND", "批次不存在或已被删除。"))?;
    Ok(Json(crate::api::dto::batch_json(&batch)))
}

/// `GET /api/batches/{id}/tasks`
async fn batch_tasks(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let items = state.tasks.list_tasks_by_batch(&id).await.api("读取批次任务失败")?;
    Ok(Json(json!(crate::api::dto::tasks_json(&items))))
}

/// `POST /api/batches/{id}/cancel`
async fn cancel_batch(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cancelled = state.tasks.cancel_batch(&id).await.api("取消批次失败")?;
    Ok(Json(json!({ "cancelled": cancelled })))
}

/// `POST /api/batches/{id}/retry-failed`
async fn retry_failed(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (retried, queued) = state.tasks.retry_failed_in_batch(&id).await.api("重试失败项失败")?;
    Ok(Json(json!({ "retried": retried, "queued": queued })))
}

/// `DELETE /api/batches/{id}`
async fn delete_batch(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let deleted_tasks = state.tasks.delete_batch(&id).await.api("删除批次失败")?;
    Ok(Json(json!({ "deletedTasks": deleted_tasks })))
}

// ────────────────────────────────────────────────────────────────────────────
// 历史
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CleanupBody {
    #[serde(default)]
    retention_days: Option<i32>,
    /// 直接给截止时间（优先于 `retentionDays`）
    #[serde(default)]
    older_than: Option<String>,
    #[serde(default)]
    all: bool,
}

/// `POST /api/history/cleanup`
async fn cleanup_history(
    State(state): State<AppState>,
    Json(body): Json<CleanupBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if body.all {
        let deleted_tasks = state.tasks.clear_history().await.api("清空历史失败")?;
        return Ok(Json(json!({ "deletedTasks": deleted_tasks, "deletedBatches": 0 })));
    }

    let days = match body.retention_days {
        Some(days) => days,
        None => {
            // 允许直接传截止时间：换算成天数，保持 TaskService 的接口不变
            let older_than = body
                .older_than
                .as_deref()
                .ok_or_else(|| ApiError::bad_request("CLEANUP_INVALID", "请提供 retentionDays、olderThan 或 all。"))?;
            let cutoff = chrono::DateTime::parse_from_rfc3339(older_than)
                .map_err(|_| ApiError::bad_request("CLEANUP_INVALID", "olderThan 必须是 RFC3339 时间。"))?;
            let days = (chrono::Utc::now() - cutoff.with_timezone(&chrono::Utc)).num_days();
            i32::try_from(days.max(0)).unwrap_or(0)
        }
    };

    let (deleted_tasks, deleted_batches) =
        state.tasks.cleanup_history(days).await.api("清理历史失败")?;
    Ok(Json(json!({ "deletedTasks": deleted_tasks, "deletedBatches": deleted_batches })))
}

/// `POST /api/history/clear`
async fn clear_history(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let deleted_tasks = state.tasks.clear_history().await.api("清空历史失败")?;
    Ok(Json(json!({ "deletedTasks": deleted_tasks })))
}
