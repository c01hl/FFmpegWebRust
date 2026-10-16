//! 进程内/接口层使用的轻量数据结构（不落库）。

use serde::{Deserialize, Serialize};

use super::enums::{TaskStatus, TemplateIssueSeverity};

/// 模板校验问题
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateIssue {
    pub severity: TemplateIssueSeverity,
    /// 用户可见的中文说明
    pub message: String,
    /// 关联字段（可选，界面用来高亮）
    pub field: Option<String>,
}

impl TemplateIssue {
    pub fn info(message: impl Into<String>, field: Option<&str>) -> Self {
        Self { severity: TemplateIssueSeverity::Info, message: message.into(), field: field.map(String::from) }
    }

    pub fn warning(message: impl Into<String>, field: Option<&str>) -> Self {
        Self { severity: TemplateIssueSeverity::Warning, message: message.into(), field: field.map(String::from) }
    }

    pub fn error(message: impl Into<String>, field: Option<&str>) -> Self {
        Self { severity: TemplateIssueSeverity::Error, message: message.into(), field: field.map(String::from) }
    }
}

/// 模板试运行结果
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateTestResult {
    pub success: bool,
    /// 用户可见的中文说明
    pub message: String,
    /// 实际执行的命令（完整可复制）
    pub command: String,
    /// ffmpeg 输出（尾部截断）
    pub output: String,
    /// 耗时毫秒（参考实现用 TimeSpan，这里统一成毫秒数字，见 backend-data.md §2.7）
    pub duration_ms: u64,
}

/// 任务状态变更事件（WebSocket 推送用）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusEvent {
    pub task_id: String,
    pub old_status: TaskStatus,
    pub new_status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// 任务进度事件（WebSocket 推送用）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskProgressEvent {
    pub task_id: String,
    pub progress: f64,
    pub speed: Option<f64>,
    pub eta: Option<f64>,
    /// 已处理到的秒数
    pub current_time: f64,
}

/// 任务日志事件（WebSocket 推送用）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskLogEvent {
    pub task_id: String,
    pub line: String,
}
