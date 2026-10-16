//! 模板导入 / 导出：与参考实现（C#/.NET `System.Text.Json`）**逐字一致**的 JSON 形状。
//!
//! 规格：`docs/reference/templates.md` §9（键名、大小写、结构、跳过规则、有损字段集）。
//!
//! # 与参考实现的键名约定（重要）
//!
//! 导出文件里**没有任何 camelCase 键**：键名是 C# 属性名（`Name` / `CommandArgs` / `HardwareMode` …），
//! 且 `System.Text.Json` 默认 `PropertyNameCaseInsensitive = false` —— 因此 `{"templates":[…]}`（小写）
//! 在参考实现里会「导入 0 个且不报错」。本模块的 serde 结构体只用 `#[serde(rename = "...")]`
//! 精确匹配大写键，**保持同样的语义**。
//!
//! 枚举（`HardwareMode` / `Kind`）在导出文件里是**数字**（0/1/2、0..3），这与 HTTP API 的
//! 字符串枚举（`"Auto"` / `"Select"`）是两套表示，见 `api-contract.md` §0。
//!
//! # 导出是**有损**的（刻意保持参考格式，不扩展 schema）
//!
//! 导出**不含**这些字段（全部在导入时回落默认值）：
//!
//! | 字段 | 导入后的值 |
//! |---|---|
//! | `Id` | 新生成的 24 位十六进制 id |
//! | `Type` | `User`（`asSystemTemplate = true` 时为 `System`） |
//! | `RequiresHardwareAcceleration` | 由 `HardwareMode == Required` 推导 |
//! | `RequiredFilters` | `[]`（**丢失**） |
//! | `IsRecommended` | `false`（**丢失**） |
//! | `PresetVersion`（每条的） | `0`（`asSystemTemplate = true` 时为当前预设版本） |
//! | `CreatedAt` / `UpdatedAt` | 导入时刻 |
//! | `SortOrder` 之外的展示位次信息 | — |
//!
//! 因此「导出 → 导入」**不是无损往返**；参考实现的需求里所谓 round-trip 兼容指的是
//! 「自己导出的文件能被自己原样导入且不报错」（§9.5）。这里保持同一取舍：**不新增键**，
//! 以免破坏与参考实现的互操作。

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::models::enums::{HardwareAccelerationMode, TemplateParameterKind};
use crate::models::{CommandTemplate, TemplateParameter, TemplateParameterOption, new_id};

/// 导出文件的固定标识（参考实现逐字）
pub const EXPORT_FORMAT: &str = "FFmpegWebUI.Templates";

/// 导出文件的 schema 版本（固定 1；导入时不校验，§9.1）
pub const EXPORT_SCHEMA_VERSION: i32 = 1;

/// 导出文件顶层（§9.1），**仅用于序列化**。
#[derive(Debug, Clone, Serialize)]
pub struct ExportDocument {
    #[serde(rename = "Format")]
    pub format: String,
    #[serde(rename = "SchemaVersion")]
    pub schema_version: i32,
    #[serde(rename = "ExportedAt")]
    pub exported_at: String,
    #[serde(rename = "AppVersion")]
    pub app_version: String,
    #[serde(rename = "PresetVersion")]
    pub preset_version: i32,
    #[serde(rename = "Templates")]
    pub templates: Vec<ExportItem>,
}

/// 导出文件的单项（§9.2）。
///
/// `Option<…>` + `skip_serializing_if` 表达「参考实现里可空、为 null 时整个键消失」的字段。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExportItem {
    #[serde(rename = "Name", default)]
    pub name: String,
    #[serde(rename = "Description", default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "CommandArgs", default)]
    pub command_args: String,
    #[serde(rename = "Category", default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(rename = "SupportedInputFormats", default, skip_serializing_if = "Option::is_none")]
    pub supported_input_formats: Option<Vec<String>>,
    #[serde(rename = "OutputExtension", default, skip_serializing_if = "Option::is_none")]
    pub output_extension: Option<String>,
    #[serde(rename = "TargetCodec", default, skip_serializing_if = "Option::is_none")]
    pub target_codec: Option<String>,
    /// 数字（0 = None / 1 = Auto / 2 = Required）
    #[serde(rename = "HardwareMode", default)]
    pub hardware_mode: i32,
    #[serde(rename = "RequiredEncoder", default, skip_serializing_if = "Option::is_none")]
    pub required_encoder: Option<String>,
    #[serde(rename = "Parameters", default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Vec<ExportParameter>>,
    #[serde(rename = "Tags", default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(rename = "SortOrder", default)]
    pub sort_order: i32,
}

/// 导出文件里的参数项（即 `TemplateParameter` 的原样序列化，§9.3）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExportParameter {
    #[serde(rename = "Name", default)]
    pub name: String,
    #[serde(rename = "Label", default)]
    pub label: String,
    #[serde(rename = "Description", default)]
    pub description: String,
    /// 数字（0 = Select / 1 = Number / 2 = Text / 3 = Boolean）
    #[serde(rename = "Kind", default)]
    pub kind: i32,
    #[serde(rename = "DefaultValue", default)]
    pub default_value: String,
    #[serde(rename = "Options", default)]
    pub options: Vec<ExportOption>,
    #[serde(rename = "Min", default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(rename = "Max", default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// 缺键时回落到 C# 的属性初始值 `"1"`
    #[serde(rename = "TrueValue", default = "default_true_value")]
    pub true_value: String,
    #[serde(rename = "FalseValue", default)]
    pub false_value: String,
}

/// 导出文件里的下拉选项（三个键始终输出）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExportOption {
    #[serde(rename = "Value", default)]
    pub value: String,
    #[serde(rename = "Label", default)]
    pub label: String,
    #[serde(rename = "Description", default)]
    pub description: String,
}

/// `TemplateParameter.TrueValue` 的 C# 默认值
fn default_true_value() -> String {
    "1".to_string()
}

impl ExportParameter {
    /// 模板参数 → 导出项
    pub fn from_parameter(parameter: &TemplateParameter) -> Self {
        Self {
            name: parameter.name.clone(),
            label: parameter.label.clone(),
            description: parameter.description.clone(),
            kind: parameter.kind.to_int() as i32,
            default_value: parameter.default_value.clone(),
            options: parameter
                .options
                .iter()
                .map(|option| ExportOption {
                    value: option.value.clone(),
                    label: option.label.clone(),
                    description: option.description.clone(),
                })
                .collect(),
            min: parameter.min,
            max: parameter.max,
            true_value: parameter.true_value.clone(),
            false_value: parameter.false_value.clone(),
        }
    }

    /// 导出项 → 模板参数
    pub fn into_parameter(self) -> TemplateParameter {
        TemplateParameter {
            name: self.name,
            label: self.label,
            description: self.description,
            kind: TemplateParameterKind::from_int(self.kind as i64),
            default_value: self.default_value,
            options: self
                .options
                .into_iter()
                .map(|option| TemplateParameterOption {
                    value: option.value,
                    label: option.label,
                    description: option.description,
                })
                .collect(),
            min: self.min,
            max: self.max,
            true_value: self.true_value,
            false_value: self.false_value,
        }
    }
}

impl ExportItem {
    /// 模板 → 导出项（§9.2 的字段取舍：空列表/空串按「省略键」处理）
    pub fn from_template(template: &CommandTemplate) -> Self {
        Self {
            name: template.name.clone(),
            description: Some(template.description.clone()),
            command_args: template.command_args.clone(),
            category: Some(template.category.clone()),
            supported_input_formats: if template.supported_input_formats.is_empty() {
                None
            } else {
                Some(template.supported_input_formats.clone())
            },
            output_extension: Some(template.output_extension.clone()),
            target_codec: Some(template.target_codec.clone()),
            hardware_mode: template.hardware_mode.to_int() as i32,
            required_encoder: template.required_encoder.clone(),
            parameters: if template.parameters.is_empty() {
                None
            } else {
                Some(template.parameters.iter().map(ExportParameter::from_parameter).collect())
            },
            tags: if template.tags.is_empty() { None } else { Some(template.tags.clone()) },
            sort_order: template.sort_order,
        }
    }

    /// 导出项 → 模板（§9.6 的逐项规则；不写库，调用方负责规范化与唯一名）
    ///
    /// * `as_system = false` → `User`，`PresetVersion = 0`
    /// * `as_system = true`  → `System`，`PresetVersion = 当前预设版本`
    /// * 分类空白 → `"导入"`；`null` 列表 → `[]`；`null` 字符串 → `""`
    /// * `RequiresHardwareAcceleration` 由 `HardwareMode == Required` 推导
    /// * `Id` 一律新生成（参考实现的文件里 id 随机且可能与本库冲突）
    pub fn into_template(self, as_system: bool, preset_version: i32, now: DateTime<Utc>) -> CommandTemplate {
        let hardware_mode = HardwareAccelerationMode::from_int(self.hardware_mode as i64);
        CommandTemplate {
            id: new_id(),
            name: self.name,
            description: self.description.unwrap_or_default(),
            command_args: self.command_args,
            r#type: if as_system {
                crate::models::enums::TemplateType::System
            } else {
                crate::models::enums::TemplateType::User
            },
            category: match self.category {
                Some(category) if !category.trim().is_empty() => category,
                _ => "导入".to_string(),
            },
            supported_input_formats: self.supported_input_formats.unwrap_or_default(),
            output_extension: self.output_extension.unwrap_or_default(),
            target_codec: self.target_codec.unwrap_or_default(),
            hardware_mode,
            requires_hardware_acceleration: hardware_mode == HardwareAccelerationMode::Required,
            required_encoder: self.required_encoder,
            parameters: self
                .parameters
                .unwrap_or_default()
                .into_iter()
                .map(ExportParameter::into_parameter)
                .collect(),
            // RequiredFilters / IsRecommended 不在格式里：回落默认值（§9.5 说明的有损项）
            required_filters: Vec::new(),
            tags: self.tags.unwrap_or_default(),
            is_recommended: false,
            preset_version: if as_system { preset_version } else { 0 },
            created_at: now,
            updated_at: now,
            sort_order: self.sort_order,
        }
    }
}

/// 一组模板 → 导出文档（`PresetVersion` 由调用方传入，避免本模块依赖预设目录）。
pub fn export_document(
    templates: &[CommandTemplate],
    app_version: &str,
    preset_version: i32,
) -> ExportDocument {
    ExportDocument {
        format: EXPORT_FORMAT.to_string(),
        schema_version: EXPORT_SCHEMA_VERSION,
        exported_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        app_version: app_version.to_string(),
        preset_version,
        templates: templates.iter().map(ExportItem::from_template).collect(),
    }
}

/// 取出顶层 `Templates` 数组（**键名大小写敏感**，与参考实现一致）。
///
/// * 顶层不是对象 / `Templates` 不是数组 → `Err`（参考实现抛
///   `InvalidOperationException("模板文件不是有效的 JSON：" + 原始消息)`）
/// * 缺键或 `null` → 空列表（参考实现的属性初始值是空列表，因此结果为「导入 0 个」且不报错）
/// * 小写 `templates` 不会被识别 → 空列表（复现参考实现的 `PropertyNameCaseInsensitive = false`）
pub fn document_items(value: &serde_json::Value) -> Result<Vec<serde_json::Value>, String> {
    let object = value.as_object().ok_or_else(|| "顶层不是 JSON 对象".to_string())?;
    match object.get("Templates") {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(items)) => Ok(items.clone()),
        Some(_) => Err("Templates 不是数组".to_string()),
    }
}

/// 单个数组元素 → 导出项（**逐项容错**：坏元素只影响它自己）。
///
/// PORT NOTE：参考实现对类型不匹配是**整份文件失败**（`JsonException` 直接冒泡）。
/// 这里改成逐项报错（写进 `ImportResult.errors`），其余条目照常导入——
/// 导入是「尽力而为」的操作，一条坏数据不应该让整份文件作废。
pub fn item_from_value(value: &serde_json::Value) -> Result<ExportItem, String> {
    serde_json::from_value(value.clone()).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 导出形状：键名逐字、枚举为数字、空列表省略
    #[test]
    fn export_shape_matches_reference_format() {
        let mut template = CommandTemplate {
            name: "测试".to_string(),
            command_args: "-i \"{input}\" -c:v libx264 \"{output}\"".to_string(),
            output_extension: "mp4".to_string(),
            ..Default::default()
        };
        template.parameters.push(TemplateParameter {
            name: "crf".to_string(),
            label: "CRF".to_string(),
            kind: TemplateParameterKind::Number,
            default_value: "23".to_string(),
            min: Some(0.0),
            ..Default::default()
        });

        let document = export_document(&[template], "0.1.0", 4);
        let value = serde_json::to_value(&document).expect("序列化");
        assert_eq!(value["Format"], "FFmpegWebUI.Templates");
        assert_eq!(value["SchemaVersion"], 1);
        assert_eq!(value["PresetVersion"], 4);
        let item = &value["Templates"][0];
        assert_eq!(item["Name"], "测试");
        assert_eq!(item["HardwareMode"], 0, "枚举必须是数字");
        assert!(item.get("RequiredEncoder").is_none(), "None → 键消失");
        assert!(item.get("SupportedInputFormats").is_none(), "空列表 → 键消失");
        assert_eq!(item["Parameters"][0]["Kind"], 1, "参数 Kind 也是数字");
        assert_eq!(item["Parameters"][0]["TrueValue"], "1");
        // 中文不转义
        let text = serde_json::to_string(&document).expect("字符串化");
        assert!(text.contains("测试"), "中文不应被转义：{text}");
    }

    /// 缺键回落 C# 的属性初始值
    #[test]
    fn missing_keys_fall_back_to_csharp_defaults() {
        let item: ExportItem =
            serde_json::from_str(r#"{"Name":"a","CommandArgs":"-i {input} {output}"}"#)
                .expect("解析");
        let template = item.into_template(false, 4, Utc::now());
        assert_eq!(template.category, "导入", "空分类 → 导入");
        assert_eq!(template.preset_version, 0);
        assert_eq!(template.supported_input_formats, Vec::<String>::new());
    }

    /// 顶层键名大小写敏感（小写 templates 不被识别 → 0 条且不报错）
    #[test]
    fn document_keys_are_case_sensitive() {
        let value: serde_json::Value =
            serde_json::from_str(r#"{"templates":[{"Name":"a"}]}"#).expect("解析");
        assert!(document_items(&value).expect("顶层是对象").is_empty());
        let value: serde_json::Value = serde_json::from_str(r#"{"Format":"x"}"#).expect("解析");
        assert!(document_items(&value).expect("缺 Templates 键").is_empty());
        let value = serde_json::Value::Array(vec![]);
        assert!(document_items(&value).is_err(), "顶层不是对象 → 错误");
    }
}
