//! 模板子系统：内置预设目录 + 模板服务（CRUD、校验、试运行、导入导出、系统同步）。
//!
//! 行为规格见 `docs/reference/templates.md`。
//!
//! ⚠️ 本文件与 `presets.rs` / `service.rs` 目前是**接口占位**，由实现方整体替换。

pub mod presets;
pub mod service;

pub use service::{SystemSyncResult, TemplateService};
