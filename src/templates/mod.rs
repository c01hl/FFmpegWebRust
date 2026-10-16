//! 模板子系统：内置预设目录 + 模板服务（CRUD、校验、试运行、导入导出、系统同步）。
//!
//! 行为规格见 `docs/reference/templates.md`。
//!
//! | 模块 | 内容 |
//! |---|---|
//! | [`presets`] | 51 个内置模板 + 稳定 key + `PRESET_VERSION`（§11 / §12） |
//! | [`service`] | `TemplateService`：CRUD / 校验 / 试运行 / 导入导出 / 同步（§1、§6–§10） |
//! | [`import_export`] | 与参考实现一致的导入导出 JSON 形状（§9） |

pub mod import_export;
pub mod presets;
pub mod service;

pub use import_export::{
    EXPORT_FORMAT, EXPORT_SCHEMA_VERSION, ExportDocument, ExportItem, ExportOption,
    ExportParameter,
};
pub use presets::{CATEGORY_ORDER, PRESET_VERSION, SYSTEM_KEYS};
pub use service::{
    CategoryCount, ImportResult, SystemSyncResult, TemplateAvailability, TemplateListItem,
    TemplateService, normalize_template,
};
