//! 命令模板（对应参考实现 `Models/CommandTemplate.cs`）。

use serde::{Deserialize, Serialize};

use super::enums::{HardwareAccelerationMode, TemplateParameterKind, TemplateType};

/// FFmpeg 命令模板
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct CommandTemplate {
    pub id: String,
    /// 模板名称（唯一）
    pub name: String,
    /// 模板描述
    pub description: String,
    /// FFmpeg 命令参数模板（支持占位符）
    pub command_args: String,
    /// 系统预设 / 用户自定义
    pub r#type: TemplateType,
    /// 模板分类：视频转码 / 音频提取 / 压缩 / 分辨率 / 硬件加速 …
    pub category: String,
    /// 支持的输入格式（如 `["mp4","avi"]`，空 = 不限）
    pub supported_input_formats: Vec<String>,
    /// 默认输出扩展名（不含点）
    pub output_extension: String,
    /// 目标视频编码格式（h264/hevc/av1/vp9/vp8/prores），用于在 `{encoder}` 下挑编码器
    pub target_codec: String,
    /// 是否需要硬件加速（旧字段，保留兼容）
    pub requires_hardware_acceleration: bool,
    /// 硬件加速策略
    pub hardware_mode: HardwareAccelerationMode,
    /// 所需编码器：族名（`nvenc`…）或具体名（`h264_nvenc`）
    pub required_encoder: Option<String>,
    /// 模板声明的可调参数
    pub parameters: Vec<TemplateParameter>,
    /// 模板必须用到的滤镜（如 `subtitles`、`drawtext`）
    pub required_filters: Vec<String>,
    /// 搜索标签
    pub tags: Vec<String>,
    /// 是否推荐（模板列表优先展示）
    pub is_recommended: bool,
    /// 系统模板预设版本号，用于随应用升级同步内置模板
    pub preset_version: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// 排序权重（越小越靠前）
    pub sort_order: i32,
}

impl Default for CommandTemplate {
    fn default() -> Self {
        let now = chrono::Utc::now();
        Self {
            id: crate::models::new_id(),
            name: String::new(),
            description: String::new(),
            command_args: String::new(),
            r#type: TemplateType::User,
            category: String::new(),
            supported_input_formats: Vec::new(),
            output_extension: String::new(),
            target_codec: String::new(),
            requires_hardware_acceleration: false,
            hardware_mode: HardwareAccelerationMode::None,
            required_encoder: None,
            parameters: Vec::new(),
            required_filters: Vec::new(),
            tags: Vec::new(),
            is_recommended: false,
            preset_version: 0,
            created_at: now,
            updated_at: now,
            sort_order: 0,
        }
    }
}

impl CommandTemplate {
    /// 用于展示的输入格式描述
    pub fn input_formats_display(&self) -> String {
        if self.supported_input_formats.is_empty() {
            "任意格式".to_string()
        } else {
            self.supported_input_formats.join(" / ")
        }
    }

    /// 是否为系统预设
    pub fn is_system(&self) -> bool {
        self.r#type == TemplateType::System
    }

    /// 声明了「必须使用指定硬件」时返回要求的编码器/族名
    pub fn required_hardware(&self) -> Option<&str> {
        match self.hardware_mode {
            HardwareAccelerationMode::Required => self.required_encoder.as_deref(),
            _ => self.required_encoder.as_deref(),
        }
    }
}

/// 模板可调参数定义
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct TemplateParameter {
    /// 占位符名称（不含花括号），如 `crf`
    pub name: String,
    /// 界面显示名称
    pub label: String,
    /// 帮助说明
    pub description: String,
    /// 控件类型
    pub kind: TemplateParameterKind,
    /// 默认值
    pub default_value: String,
    /// 可选项（`kind = Select` 时使用）
    pub options: Vec<TemplateParameterOption>,
    /// 数字下界
    pub min: Option<f64>,
    /// 数字上界
    pub max: Option<f64>,
    /// 布尔为 true 时替换成的文本
    pub true_value: String,
    /// 布尔为 false 时替换成的文本（空字符串表示移除该片段）
    pub false_value: String,
}

impl Default for TemplateParameter {
    fn default() -> Self {
        Self {
            name: String::new(),
            label: String::new(),
            description: String::new(),
            kind: TemplateParameterKind::Select,
            default_value: String::new(),
            options: Vec::new(),
            min: None,
            max: None,
            true_value: "1".to_string(),
            false_value: String::new(),
        }
    }
}

/// 模板参数选项
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct TemplateParameterOption {
    /// 提交给 FFmpeg 的值
    pub value: String,
    /// 界面显示文本
    pub label: String,
    /// 补充说明
    pub description: String,
}

impl Default for TemplateParameterOption {
    fn default() -> Self {
        Self {
            value: String::new(),
            label: String::new(),
            description: String::new(),
        }
    }
}

impl TemplateParameterOption {
    pub fn new(value: impl Into<String>, label: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: description.into(),
        }
    }
}
