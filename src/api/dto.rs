//! 接口层的 JSON 投影工具。
//!
//! 契约（`docs/api-contract.md` §0.1 / §1.5）要求任务对象里带上**计算字段**
//! `durationSeconds` 与 `logLines`。它们不是数据库列，所以在这里统一补：
//! REST 响应与 WebSocket 事件都走同一个函数，避免两边算出不同的值。

use serde_json::{json, Map, Value};

use crate::models::{BatchTask, ConversionTask};

/// 单个任务 → 契约要求的 JSON
pub fn task_json(task: &ConversionTask) -> Value {
    let mut value = serde_json::to_value(task).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.insert("durationSeconds".to_string(), json!(task.duration_seconds()));
        object.insert("logLines".to_string(), json!(task.log_lines()));
    }
    value
}

/// 任务列表 → JSON 数组
pub fn tasks_json(tasks: &[ConversionTask]) -> Vec<Value> {
    tasks.iter().map(task_json).collect()
}

/// 批次 → 契约要求的 JSON（目前没有计算字段，保留成函数是为了 REST 与 WS 一致）
pub fn batch_json(batch: &BatchTask) -> Value {
    serde_json::to_value(batch).unwrap_or(Value::Null)
}

/// 给已经序列化好的对象补上任务的计算字段（WebSocket 事件里内嵌的 `task` 用）。
///
/// 入参是事件信封的 `data`，若其中有 `task` 对象就补 `durationSeconds` / `logLines`。
pub fn enrich_embedded_task(data: &mut Value) {
    let Some(object) = data.as_object_mut() else { return };
    let Some(task) = object.get_mut("task") else { return };
    let Some(task_object) = task.as_object_mut() else { return };

    if !task_object.contains_key("durationSeconds") || task_object["durationSeconds"].is_null() {
        let duration = duration_from_object(task_object);
        task_object.insert("durationSeconds".to_string(), json!(duration));
    }
}

/// 从任务对象的 `startedAt` / `completedAt` 反算耗时（RFC3339）
fn duration_from_object(task: &Map<String, Value>) -> Option<f64> {
    let started = task.get("startedAt").and_then(Value::as_str)?;
    let completed = task.get("completedAt").and_then(Value::as_str)?;

    let started = chrono::DateTime::parse_from_rfc3339(started).ok()?;
    let completed = chrono::DateTime::parse_from_rfc3339(completed).ok()?;
    let millis = (completed - started).num_milliseconds();
    Some(millis as f64 / 1000.0)
}

/// 硬件检测报告 → 契约里的 `HardwareReportDto`。
///
/// 为什么要投影而不是直接 `serde_json::to_value`：
/// `ffmpeg::hardware` 里的 `HardwareReport` / `DetectedEncoder` 是**执行层结构**，
/// 字段名（`probed_at` / `encoder_id` / `verified_by_trial` …）与契约要求的 camelCase
/// 名字不一致。在这里做一次显式映射，前端拿到的就是 `camelCase` 的稳定形状；
/// 以后执行层改字段名也不会静默破坏接口。
pub fn hardware_report_json(report: &crate::ffmpeg::hardware::HardwareReport) -> Value {
    let encoders: Vec<Value> = report
        .encoders
        .iter()
        .map(|encoder| {
            json!({
                // 前端用它做列表 key：编码器名本身唯一且稳定
                "id": encoder.encoder_id,
                "name": encoder.encoder_id,
                "displayName": if encoder.display_name.trim().is_empty() {
                    encoder.encoder_name.clone()
                } else {
                    encoder.display_name.clone()
                },
                "type": encoder.encoder_type,
                "family": encoder.family,
                "codec": encoder.target_codec,
                "isAvailable": encoder.available,
                "supportsQualityOptions": encoder.verified_by_trial,
                "isHardware": encoder.is_hardware,
                "lastCheckedAt": encoder.last_checked_at,
                "unavailableReason": if encoder.available || encoder.reason.trim().is_empty() {
                    Value::Null
                } else {
                    json!(encoder.reason)
                },
                "missingFromBuild": encoder.missing_from_build,
                "ffmpegVersion": encoder.ffmpeg_version,
                "verifiedByTrial": encoder.verified_by_trial,
                "trialCommand": encoder.trial_command,
                // 兼容旧字段：参考实现里恒为 [codec]
                "supportedCodecs": [encoder.target_codec],
            })
        })
        .collect();

    json!({
        "detectedAt": report.probed_at,
        "ffmpegVersion": report.ffmpeg_version,
        "ffmpegPath": report.ffmpeg_path,
        "platform": report.platform,
        "gpuSummary": report.gpu_summary,
        "hwaccels": report.hwaccels,
        "supportedFilters": report.supported_filters,
        "summary": report.summary,
        "recommended": report.recommended,
        "encoders": encoders,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::enums::TaskStatus;

    #[test]
    fn task_json_adds_computed_fields() {
        let mut task = ConversionTask::default();
        task.status = TaskStatus::Completed;
        task.started_at = Some(chrono::Utc::now() - chrono::Duration::seconds(3));
        task.completed_at = Some(chrono::Utc::now());
        task.log_output = "a\nb\n".to_string();

        let value = task_json(&task);
        assert_eq!(value["logLines"], 2);
        let duration = value["durationSeconds"].as_f64().expect("应有耗时");
        assert!((2.5..=3.5).contains(&duration), "{duration}");
        // 原有字段必须保留
        assert_eq!(value["status"], "Completed");
    }

    #[test]
    fn task_json_has_null_duration_when_not_finished() {
        let task = ConversionTask::default();
        let value = task_json(&task);
        assert!(value["durationSeconds"].is_null());
        assert_eq!(value["logLines"], 0);
    }

    #[test]
    fn enrich_adds_duration_to_embedded_task() {
        let mut payload = json!({
            "taskId": "x",
            "task": {
                "id": "x",
                "status": "Completed",
                "startedAt": "2026-02-14T03:12:45.001Z",
                "completedAt": "2026-02-14T03:13:20.456Z"
            }
        });

        enrich_embedded_task(&mut payload);
        let duration = payload["task"]["durationSeconds"].as_f64().expect("应有耗时");
        assert!((35.3..=35.6).contains(&duration), "{duration}");
    }

    #[test]
    fn enrich_is_a_noop_without_task_object() {
        let mut payload = json!({ "queued": 1 });
        enrich_embedded_task(&mut payload);
        assert_eq!(payload["queued"], 1);
    }
}
