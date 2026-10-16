//! WebSocket 接口（`docs/api-contract.md` §2）。
//!
//! 设计要点：
//! - **单连接、广播式**：服务端不区分会话，所有订阅者收到同一份事件流；
//! - 连接建立后**立即推一份快照**，避免首屏竞态（前端断线重连也依赖它恢复）；
//! - 服务端 90 秒没收到任何消息就断开（前端每 20 秒发一次 `ping`）；
//! - `task.log` 在队列层已经按 100–200ms 合并过，这里不再做二次缓冲。

use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::broadcast::error::RecvError;

use crate::queue::QueueEvent;
use crate::state::AppState;

/// 客户端静默多久后断开（前端心跳间隔 20 秒，留足余量）
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);

pub fn routes() -> Router<AppState> {
    Router::new().route("/ws", get(upgrade))
}

async fn upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle(socket, state))
}

async fn handle(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    // 1) 先推快照，避免前端在收到第一条事件前看到空列表
    let snapshot = build_snapshot(&state).await;
    if sender.send(Message::Text(snapshot.to_string().into())).await.is_err() {
        return;
    }

    let mut events = state.queue.subscribe();

    loop {
        tokio::select! {
            // 队列事件 → 前端
            event = events.recv() => {
                match event {
                    Ok(event) => {
                        let payload = envelope(&event);
                        if sender.send(Message::Text(payload.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(dropped)) => {
                        // 前端跟不上时不能报错退出：补一份快照即可（日志行本来就是增量的）
                        tracing::debug!("WebSocket 订阅者落后，丢弃 {dropped} 条事件后补发快照");
                        let snapshot = build_snapshot(&state).await;
                        if sender.send(Message::Text(snapshot.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Closed) => break,
                }
            }

            // 客户端消息
            incoming = receiver.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if !handle_client_message(&text, &state, &mut sender).await {
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if sender.send(Message::Pong(payload)).await.is_err() { break; }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }

            // 心跳超时：前端没在约定时间内发消息就断开，避免僵尸连接堆积
            _ = tokio::time::sleep(IDLE_TIMEOUT) => {
                tracing::debug!("WebSocket 客户端静默超时，断开连接");
                break;
            }
        }
    }
}

/// 返回 `false` 表示应当关闭连接
async fn handle_client_message(
    text: &str,
    state: &AppState,
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
) -> bool {
    let Ok(message) = serde_json::from_str::<Value>(text) else {
        return true; // 坏消息忽略，不断开
    };

    match message.get("action").and_then(Value::as_str) {
        Some("ping") => sender
            .send(Message::Text(json!({ "type": "pong", "ts": now_iso(), "data": {} }).to_string().into()))
            .await
            .is_ok(),
        Some("task.cancel") => {
            let task_id = message.get("taskId").and_then(Value::as_str).unwrap_or_default();
            let graceful = message.get("graceful").and_then(Value::as_bool).unwrap_or(false);
            if !task_id.is_empty() {
                if let Err(error) = state.tasks.cancel_task(task_id, graceful).await {
                    tracing::debug!("WebSocket 取消任务失败：{error}");
                }
            }
            true
        }
        // 订阅是按主题过滤的预留能力：当前所有事件都转发，忽略即可
        Some("subscribe") | None => true,
        Some(_) => true,
    }
}

/// 连接建立（以及落后重连）时推送的快照
async fn build_snapshot(state: &AppState) -> Value {
    let running = state.tasks.running_tasks().await.unwrap_or_default();
    let pending = state.tasks.pending_tasks().await.unwrap_or_default();
    let max_concurrent = state.settings.cached().await.max_concurrent_tasks;

    json!({
        "type": "snapshot",
        "ts": now_iso(),
        "data": {
            "queue": {
                "queued": state.queue.queued_count(),
                "active": state.queue.active_count(),
                "maxConcurrentTasks": max_concurrent,
            },
            "runningTasks": crate::api::dto::tasks_json(&running),
            "pendingTasks": crate::api::dto::tasks_json(&pending),
        }
    })
}

/// 把队列内部事件包成契约里的信封：`{"type","ts","data"}`
///
/// 队列事件的变体名是 `progress` / `status` / `log` / `queueChanged` / `batchStatus`，
/// 契约里对外的名字是 `task.progress` / `task.status` / `task.log` / `queue.changed` / `batch.status`，
/// 这里做一次显式映射（不要在两端各自猜名字）。
fn envelope(event: &QueueEvent) -> Value {
    let mut value = serde_json::to_value(event).unwrap_or(Value::Null);
    let mut data = Value::Object(serde_json::Map::new());

    if let Some(object) = value.as_object_mut() {
        if let Some(Value::String(kind)) = object.remove("type") {
            let mapped = match kind.as_str() {
                "progress" => "task.progress",
                "status" => "task.status",
                "log" => "task.log",
                "queueChanged" => "queue.changed",
                "batchStatus" => "batch.status",
                other => other,
            };
            data = Value::Object(std::mem::take(object));
            // 事件里内嵌的 task 也要带上契约要求的计算字段
            crate::api::dto::enrich_embedded_task(&mut data);
            return json!({ "type": mapped, "ts": now_iso(), "data": data });
        }
    }

    json!({ "type": "unknown", "ts": now_iso(), "data": data })
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::enums::TaskStatus;

    #[test]
    fn envelope_renames_event_types() {
        let event = QueueEvent::QueueChanged { queued: 2, active: 1 };
        let value = envelope(&event);
        assert_eq!(value["type"], "queue.changed");
        assert_eq!(value["data"]["queued"], 2);
        assert_eq!(value["data"]["active"], 1);
        assert!(value["ts"].as_str().unwrap().ends_with('Z'));
    }

    #[test]
    fn envelope_wraps_task_status_payload() {
        let mut task = crate::models::ConversionTask::default();
        task.id = "66f1c0a2b3d4e5f60718293a".to_string();
        let event = QueueEvent::Status {
            task_id: task.id.clone(),
            old_status: TaskStatus::Running,
            new_status: TaskStatus::Completed,
            error_message: None,
            task,
        };

        let value = envelope(&event);
        assert_eq!(value["type"], "task.status");
        assert_eq!(value["data"]["newStatus"], "Completed");
        assert_eq!(value["data"]["task"]["id"], "66f1c0a2b3d4e5f60718293a");
        assert!(value["data"]["task"]["durationSeconds"].is_null(), "未结束的任务耗时应为 null");
    }

    #[test]
    fn envelope_wraps_log_batches() {
        let event = QueueEvent::Log {
            task_id: "abc".to_string(),
            lines: vec!["frame=1".to_string()],
            dropped: 0,
            truncated: false,
        };
        let value = envelope(&event);
        assert_eq!(value["type"], "task.log");
        assert_eq!(value["data"]["lines"][0], "frame=1");
    }
}
