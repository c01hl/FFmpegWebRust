//! 转换任务与批次（对应参考实现 `Models/ConversionTask.cs` / `Models/BatchTask.cs`）。

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::enums::TaskStatus;

/// 单个转换任务
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionTask {
    pub id: String,
    /// 输入文件绝对路径
    pub input_path: String,
    /// 输出文件路径（执行期会被冲突解析结果覆盖）
    pub output_path: String,
    /// 模板 ID
    pub template_id: String,
    /// 模板名快照（模板被删后仍可展示）
    #[serde(default)]
    pub template_name: String,
    /// 实际执行的 ffmpeg 参数串（不含可执行文件名）
    #[serde(default)]
    pub actual_command: String,
    pub status: TaskStatus,
    /// 进度百分比 0..100
    pub progress: f64,
    /// 输入总时长（秒）
    #[serde(default)]
    pub total_duration: f64,
    /// 当前处理到的时间（秒）
    #[serde(default)]
    pub current_time: f64,
    /// 预估剩余秒数（任务结束时置 null）
    pub estimated_time_remaining: Option<f64>,
    /// 处理倍速（任务结束时置 null）
    pub processing_speed: Option<f64>,
    /// 日志文本（多行，有行数上限）
    #[serde(default)]
    pub log_output: String,
    /// 失败原因（中文）
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub input_file_size: i64,
    /// 输出字节数，仅成功时填充
    pub output_file_size: Option<i64>,
    /// 所属批次
    pub batch_id: Option<String>,
    /// 创建任务时用户选定的参数（重试时复用）
    pub template_parameters: Option<HashMap<String, String>>,
    /// 本任务由哪个任务重试而来
    pub retry_of_task_id: Option<String>,
    /// 输出路径已由界面完成冲突解析：true 时执行层不再套用全局策略
    #[serde(default)]
    pub output_path_resolved: bool,
    /// 是否允许覆盖（仅 `output_path_resolved = true` 时有意义）
    #[serde(default)]
    pub overwrite_existing: bool,
}

impl Default for ConversionTask {
    fn default() -> Self {
        Self {
            id: crate::models::new_id(),
            input_path: String::new(),
            output_path: String::new(),
            template_id: String::new(),
            template_name: String::new(),
            actual_command: String::new(),
            status: TaskStatus::Pending,
            progress: 0.0,
            total_duration: 0.0,
            current_time: 0.0,
            estimated_time_remaining: None,
            processing_speed: None,
            log_output: String::new(),
            error_message: None,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            input_file_size: 0,
            output_file_size: None,
            batch_id: None,
            template_parameters: None,
            retry_of_task_id: None,
            output_path_resolved: false,
            overwrite_existing: false,
        }
    }
}

impl ConversionTask {
    /// 实际耗时秒数（计算属性，不存储）：起止时间都有值时才返回
    pub fn duration_seconds(&self) -> Option<f64> {
        match (self.started_at, self.completed_at) {
            (Some(start), Some(end)) => Some((end - start).num_milliseconds() as f64 / 1000.0),
            _ => None,
        }
    }

    /// 日志行数
    pub fn log_lines(&self) -> usize {
        if self.log_output.is_empty() {
            0
        } else {
            self.log_output.lines().count()
        }
    }

    /// 是否处于终态
    pub fn is_terminal(&self) -> bool {
        self.status.is_terminal()
    }
}

/// 一次批量提交（N 个子任务）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchTask {
    pub id: String,
    /// 批次名，创建时固定为 `批量任务 - {yyyy-MM-dd HH:mm}`（本地时间）
    pub name: String,
    /// 批次内所有子任务共用的模板
    pub template_id: String,
    pub total_files: i32,
    pub completed_files: i32,
    /// 状态为 Failed 或 Cancelled 的子任务数量
    pub failed_files: i32,
    pub status: TaskStatus,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl Default for BatchTask {
    fn default() -> Self {
        Self {
            id: crate::models::new_id(),
            name: String::new(),
            template_id: String::new(),
            total_files: 0,
            completed_files: 0,
            failed_files: 0,
            status: TaskStatus::Pending,
            created_at: Utc::now(),
            completed_at: None,
        }
    }
}
