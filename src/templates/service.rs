//! 模板服务（接口占位，由实现方替换）。
//!
//! 契约与算法见 `docs/reference/templates.md` §6–§9。

use serde::{Deserialize, Serialize};

/// 系统模板同步结果
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemSyncResult {
    /// 新增数量
    pub added: i32,
    /// 更新数量
    pub updated: i32,
    /// 未变化数量
    pub unchanged: i32,
    /// 当前预设版本
    pub preset_version: i32,
    /// 中文摘要（日志与界面共用）
    pub summary: String,
}

/// 模板服务（接口占位）
#[derive(Clone, Default)]
pub struct TemplateService;
