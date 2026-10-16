//! 领域枚举（对应参考实现 `Models/Enums.cs`）。
//!
//! 约定：
//! - **持久化/导入导出**：一律使用整数（与参考实现的 LiteDB / 模板导出 JSON 一致，
//!   见 `docs/reference/templates.md` §1.4）。
//! - **HTTP API**：序列化为枚举名（`"Pending"`、`"Nvenc"` …），前端可读性优先。
//!
//! 数值即契约，不要调整顺序。

use serde::{Deserialize, Serialize};

/// 模板类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum TemplateType {
    /// 系统预设（不可删除，随应用版本同步）
    System = 0,
    /// 用户自定义
    #[default]
    User = 1,
}

/// 任务状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum TaskStatus {
    /// 待执行（已入库、未入队或已入队未开始）
    #[default]
    Pending = 0,
    /// 正在执行（ffmpeg 子进程存活）
    Running = 1,
    /// 已完成（ffmpeg 退出码 0）
    Completed = 2,
    /// 失败（非 0 退出 / 超时 / 模板缺失 / 目录不可写等）
    Failed = 3,
    /// 已取消
    Cancelled = 4,
    /// 已跳过（输出文件已存在且设置为跳过；**不是错误**）
    Skipped = 5,
}

/// 模板对硬件加速的依赖程度
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum HardwareAccelerationMode {
    /// 不使用硬件加速（纯软件编码）
    #[default]
    None = 0,
    /// 自动：有可用硬件编码器就优先使用，否则回退到软件编码
    Auto = 1,
    /// 必须使用指定族的硬件编码器，否则模板不可用
    Required = 2,
}

/// 输出文件命名规则
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum OutputNamingRule {
    /// 保持原名
    Original = 0,
    /// 添加后缀
    #[default]
    Suffix = 1,
}

/// 文件已存在时的处理方式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum FileExistsAction {
    /// 询问用户（单文件转换时在界面确认，批量处理时自动重命名）
    #[default]
    Ask = 0,
    /// 覆盖
    Overwrite = 1,
    /// 跳过
    Skip = 2,
    /// 重命名
    Rename = 3,
}

/// 模板参数控件类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum TemplateParameterKind {
    /// 下拉选择
    #[default]
    Select = 0,
    /// 数字输入
    Number = 1,
    /// 文本输入
    Text = 2,
    /// 布尔开关
    Boolean = 3,
}

/// 模板校验问题的严重程度
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub enum TemplateIssueSeverity {
    /// 提示
    #[default]
    Info = 0,
    /// 警告：可用，但可能不符合预期
    Warning = 1,
    /// 错误：几乎必然执行失败
    Error = 2,
}

/// 任务状态的中文显示名（界面与日志共用）
impl TaskStatus {
    pub fn display_name(&self) -> &'static str {
        match self {
            TaskStatus::Pending => "等待中",
            TaskStatus::Running => "执行中",
            TaskStatus::Completed => "已完成",
            TaskStatus::Failed => "失败",
            TaskStatus::Cancelled => "已取消",
            TaskStatus::Skipped => "已跳过",
        }
    }

    /// 是否处于终态
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Skipped
        )
    }
}

impl TemplateType {
    pub fn display_name(&self) -> &'static str {
        match self {
            TemplateType::System => "系统预设",
            TemplateType::User => "用户模板",
        }
    }
}

/// `TaskStatus` 的整数字面量（数据库列）
macro_rules! int_enum {
    ($ty:ty, $($variant:path = $value:expr),+ $(,)?) => {
        impl $ty {
            /// 转成数据库/导出 JSON 使用的整数值
            pub fn to_int(self) -> i64 {
                match self { $($variant => $value),+ }
            }

            /// 从整数值还原；未知值回退到默认值（与原实现的容错一致）
            pub fn from_int(value: i64) -> Self {
                match value { $($value => $variant,)+ _ => Self::default() }
            }
        }
    };
}

int_enum!(TemplateType, TemplateType::System = 0, TemplateType::User = 1);
int_enum!(
    TaskStatus,
    TaskStatus::Pending = 0,
    TaskStatus::Running = 1,
    TaskStatus::Completed = 2,
    TaskStatus::Failed = 3,
    TaskStatus::Cancelled = 4,
    TaskStatus::Skipped = 5
);
int_enum!(
    HardwareAccelerationMode,
    HardwareAccelerationMode::None = 0,
    HardwareAccelerationMode::Auto = 1,
    HardwareAccelerationMode::Required = 2
);
int_enum!(
    OutputNamingRule,
    OutputNamingRule::Original = 0,
    OutputNamingRule::Suffix = 1
);
int_enum!(
    FileExistsAction,
    FileExistsAction::Ask = 0,
    FileExistsAction::Overwrite = 1,
    FileExistsAction::Skip = 2,
    FileExistsAction::Rename = 3
);
int_enum!(
    TemplateParameterKind,
    TemplateParameterKind::Select = 0,
    TemplateParameterKind::Number = 1,
    TemplateParameterKind::Text = 2,
    TemplateParameterKind::Boolean = 3
);
int_enum!(
    TemplateIssueSeverity,
    TemplateIssueSeverity::Info = 0,
    TemplateIssueSeverity::Warning = 1,
    TemplateIssueSeverity::Error = 2
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_roundtrip() {
        for status in [
            TaskStatus::Pending,
            TaskStatus::Running,
            TaskStatus::Completed,
            TaskStatus::Failed,
            TaskStatus::Cancelled,
            TaskStatus::Skipped,
        ] {
            assert_eq!(TaskStatus::from_int(status.to_int()), status);
        }
        // 未知值回退默认（Pending），与原实现的容错一致
        assert_eq!(TaskStatus::from_int(99), TaskStatus::Pending);
    }

    #[test]
    fn json_uses_names() {
        assert_eq!(serde_json::to_string(&TaskStatus::Running).unwrap(), "\"Running\"");
        assert_eq!(
            serde_json::from_str::<TaskStatus>("\"Skipped\"").unwrap(),
            TaskStatus::Skipped
        );
    }
}
