//! 命令行构建（对应参考实现 `Services/CommandLineBuilder.cs`）。
//!
//! 本文件只放**类型定义**（已被模板服务、队列、API 层依赖，接口冻结）；
//! 算法实现在 `builder.rs`。

pub mod builder;

pub use builder::{build, extract_placeholders, render_command_line, tokenize};

use std::collections::HashMap;

use crate::ffmpeg::encoder_catalog::EncoderProfile;
use crate::models::enums::HardwareAccelerationMode;

/// 模板命令里可用的媒体信息摘要（与 ffmpeg 模块的重型 `MediaInfo` 解耦，
/// 便于 `command` 模块独立测试）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaSummary {
    /// 总时长（秒）
    pub duration: f64,
    pub width: i32,
    pub height: i32,
    /// 帧率（fps）
    pub frame_rate: f64,
    /// 视频编码名，如 `h264`
    pub video_codec: Option<String>,
    /// 音频编码名，如 `aac`
    pub audio_codec: Option<String>,
    pub audio_sample_rate: i32,
    pub audio_channels: i32,
    /// 是否存在视频/音频流
    pub has_video: bool,
    pub has_audio: bool,
}

impl MediaSummary {
    /// 从各字段构造（`ffmpeg::service::MediaInfo` → 本结构的转换由队列层负责）
    pub fn from_parts(
        duration: f64,
        width: i32,
        height: i32,
        frame_rate: f64,
        video_codec: Option<String>,
        audio_codec: Option<String>,
        audio_sample_rate: i32,
        audio_channels: i32,
        has_video: bool,
        has_audio: bool,
    ) -> Self {
        Self {
            duration,
            width,
            height,
            frame_rate,
            video_codec,
            audio_codec,
            audio_sample_rate,
            audio_channels,
            has_video,
            has_audio,
        }
    }
}

/// 构建一次 ffmpeg 命令所需的上下文
#[derive(Debug, Clone, Default)]
pub struct CommandContext {
    pub input_path: String,
    pub output_path: String,
    /// 用户参数值（键为模板声明的参数名；比较时大小写不敏感）
    pub parameters: HashMap<String, String>,
    /// 已解析的视频编码器画像
    pub encoder: Option<EncoderProfile>,
    /// `{audio_encoder}`，默认 `aac`
    pub audio_encoder: String,
    /// 覆盖画像默认质量
    pub quality: Option<i32>,
    pub media: Option<MediaSummary>,
    /// `0` = 不指定（`{threads}` 为空）
    pub threads: i32,
    /// 如 `-hwaccel videotoolbox`；非空时自动注入到输入之前
    pub hardware_decode_args: String,
    /// 用户设置里的全局附加参数，追加到输出之前
    pub global_arguments: String,
}

impl CommandContext {
    pub fn new(input: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            input_path: input.into(),
            output_path: output.into(),
            audio_encoder: "aac".to_string(),
            ..Self::default()
        }
    }

    /// 目标编码格式（用于挑编码器）：模板声明优先，否则由输出扩展名推断
    pub fn target_codec(template: &crate::models::CommandTemplate) -> String {
        if !template.target_codec.trim().is_empty() {
            return template.target_codec.trim().to_lowercase();
        }
        codec_from_extension(&template.output_extension).to_string()
    }
}

/// 由输出扩展名推断目标视频编码（与参考实现的推断规则一致）
pub fn codec_from_extension(extension: &str) -> &'static str {
    match extension.trim().trim_start_matches('.').to_lowercase().as_str() {
        "mp4" | "mov" | "m4v" | "mkv" => "h264",
        "webm" => "vp9",
        "avi" => "h264",
        "gif" => "gif",
        _ => "h264",
    }
}

/// 构建结果
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BuiltCommand {
    /// 展示/复制用（已按需加引号渲染）
    pub command_line: String,
    /// 真正传给 ffmpeg 的 argv（不经 shell）
    pub arguments: Vec<String>,
    pub encoder_name: Option<String>,
    /// 中文告警
    pub warnings: Vec<String>,
    /// 去重后仍未解析的占位符名（不含花括号）
    pub unresolved_placeholders: Vec<String>,
}

impl BuiltCommand {
    /// 是否需要向用户提示「这个命令可能跑不起来」
    pub fn has_problems(&self) -> bool {
        !self.unresolved_placeholders.is_empty()
    }
}

/// 模板对硬件的要求是否允许使用给定编码器（供队列与界面共用）
pub fn hardware_requirement_satisfied(
    mode: HardwareAccelerationMode,
    required_encoder: Option<&str>,
    encoder: Option<&EncoderProfile>,
) -> bool {
    match mode {
        HardwareAccelerationMode::None => true,
        HardwareAccelerationMode::Auto => true,
        HardwareAccelerationMode::Required => match (required_encoder, encoder) {
            (None, _) => true,
            (Some(required), Some(profile)) => {
                profile.name.eq_ignore_ascii_case(required)
                    || profile.family.eq_ignore_ascii_case(required)
                    || profile.encoder_type.family_matches(required)
            }
            (Some(_), None) => false,
        },
    }
}
