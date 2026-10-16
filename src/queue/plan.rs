//! 执行期的**纯计算**辅助（无 IO、无共享状态，便于单测）。
//!
//! 全部逐条对应规格：`backend-data.md` §3.6.2（目标 codec）/ §3.6.3（硬件解码）/ §3.6.4（回退判定）
//! / §3.7（进度摘要行）/ §3.8（日志截断）；`templates.md` §3.7（音频编码器与上下文装配）/ §5.2（编码器选择）。

use std::collections::HashMap;

use crate::command::{BuiltCommand, CommandContext, MediaSummary, build};
use crate::ffmpeg::encoder_catalog::{EncoderCatalog, EncoderProfile};
use crate::ffmpeg::hardware::HardwareDetector;
use crate::ffmpeg::service::{MediaInfo, ProgressTick, RunOutcome};
use crate::models::enums::HardwareAccelerationMode;
use crate::models::{CommandTemplate, ConversionTask};
use crate::paths;

/// 注入 `-hwaccel` 的输入视频编码白名单（规格 §3.6.3）
const HARDWARE_DECODE_CODECS: &[&str] = &["h264", "hevc", "av1", "vp9", "mpeg2video", "vc1"];

/// 回退判定关键字（规格 §3.6.4，逐字；忽略大小写）
pub(crate) const FALLBACK_KEYWORDS: &[&str] = &[
    "Cannot load",
    "Device creation failed",
    "No capable devices found",
    "Error initializing",
    "Cannot open device",
    "Failed to initialise",
    "Failed to initialize",
    "No device available",
    "not available",
    "Function not implemented",
    "Operation not permitted",
    "Invalid argument",
    "Unrecognized option",
    "Option not found",
    "Error while opening encoder",
    "Could not open encoder",
    "Generic error in an external library",
    "OpenEncodeSessionEx failed",
    "No NVENC capable devices found",
    "Error creating an internal MFX session",
    "unsupported",
    "MFX_ERR",
];

/// 由画像名推断目标 codec 时按此顺序扫描命令模板（规格 §3.6.2 第 4 步）
const CODEC_PROBE_ORDER: &[&str] = &["libx265", "libx264", "libsvtav1", "libvpx-vp9", "libvpx"];

// ────────────────────────────────────────────────────────────────────────────
// 目标 codec / 音频编码器
// ────────────────────────────────────────────────────────────────────────────

/// 目标视频编码格式推断（规格 §3.6.2，五步逐序）。
pub(crate) fn determine_target_codec(template: &CommandTemplate) -> String {
    // 1) 模板显式声明
    let declared = EncoderCatalog::normalize_codec(&template.target_codec);
    if !declared.is_empty() {
        return declared;
    }

    // 2) 模板要求的编码器（具体名 → 画像 codec；族名 → 该族第一个编码器的 codec）
    if let Some(required) = template
        .required_encoder
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Some(profile) = EncoderCatalog::find(required) {
            return EncoderCatalog::normalize_codec(&profile.codec);
        }
        if let Some(profile) = EncoderCatalog::by_family(required).first() {
            return EncoderCatalog::normalize_codec(&profile.codec);
        }
    }

    // 3) 输出扩展名归一后落在四种视频 codec 之一
    let from_extension = EncoderCatalog::normalize_codec(&template.output_extension);
    if matches!(from_extension.as_str(), "h264" | "hevc" | "av1" | "vp9") {
        return from_extension;
    }

    // 4) 命令模板里出现的编码器名（注意顺序：libx265 先于 libx264）
    let args = template.command_args.to_ascii_lowercase();
    for encoder in CODEC_PROBE_ORDER {
        if args.contains(encoder) {
            if let Some(profile) = EncoderCatalog::find(encoder) {
                return EncoderCatalog::normalize_codec(&profile.codec);
            }
        }
    }

    // 5) 扩展名兜底
    match template.output_extension.trim().trim_start_matches('.').to_ascii_lowercase().as_str() {
        "webm" => "vp9".to_string(),
        "mkv" | "mov" => "h264".to_string(),
        _ => "h264".to_string(),
    }
}

/// `{audio_encoder}` 的取值（`templates.md` §3.7 `ResolveAudioEncoder`）。
///
/// 顺序固定：命令模板里出现 `libopus` → `libmp3lame` → `flac` → `aac`；
/// 都没出现时按输出扩展名映射。
pub(crate) fn resolve_audio_encoder(template: &CommandTemplate) -> String {
    let args = template.command_args.to_ascii_lowercase();
    for candidate in ["libopus", "libmp3lame", "flac", "aac"] {
        if args.contains(candidate) {
            return candidate.to_string();
        }
    }
    match template.output_extension.trim().trim_start_matches('.').to_ascii_lowercase().as_str() {
        "webm" | "ogg" => "libopus".to_string(),
        "mp3" => "libmp3lame".to_string(),
        "flac" => "flac".to_string(),
        _ => "aac".to_string(),
    }
}

/// 平台默认的硬件解码器（规格 §3.6.3）。
pub(crate) fn platform_hwaccel() -> &'static str {
    if paths::is_macos() {
        "videotoolbox"
    } else if paths::is_windows() {
        "d3d11va"
    } else {
        "vaapi"
    }
}

/// 硬件解码参数（规格 §3.6.3）：不满足条件时返回空串。
///
/// 调用方负责「`settings.prefer_hardware_decoding` 且本次尝试未禁用硬件解码」这一半条件
/// （参数 `enable`），本函数负责「有视频流且编码在允许列表里」这一半。
pub(crate) fn build_hardware_decode_args(media: Option<&MediaInfo>, enable: bool) -> String {
    if !enable {
        return String::new();
    }
    let Some(video) = media.and_then(|media| media.video.as_ref()) else {
        return String::new();
    };
    if !video.is_video() {
        return String::new();
    }
    let codec = video.codec_name.trim().to_ascii_lowercase();
    if !HARDWARE_DECODE_CODECS.contains(&codec.as_str()) {
        return String::new();
    }
    format!("-hwaccel {}", platform_hwaccel())
}

// ────────────────────────────────────────────────────────────────────────────
// 参数与上下文装配
// ────────────────────────────────────────────────────────────────────────────

/// 模板默认参数值字典（`templates.md` §2.3）。
///
/// 算法归模板服务所有（空名参数不进字典），队列直接复用，避免两份实现漂移。
pub(crate) fn default_parameter_values(template: &CommandTemplate) -> HashMap<String, String> {
    crate::templates::TemplateService::default_values(template)
}

/// 默认值 + 任务上保存的用户取值（任务侧优先，`templates.md` §3.7）。
pub(crate) fn merged_parameters(
    template: &CommandTemplate,
    overrides: Option<&HashMap<String, String>>,
) -> HashMap<String, String> {
    let mut values = default_parameter_values(template);
    if let Some(overrides) = overrides {
        for (key, value) in overrides {
            if !key.trim().is_empty() {
                values.insert(key.clone(), value.clone());
            }
        }
    }
    values
}

/// `MediaInfo` → `CommandContext` 需要的轻量摘要。
pub(crate) fn media_summary(media: &MediaInfo) -> MediaSummary {
    let video = media.video.as_ref();
    let audio = media.audio.as_ref();
    MediaSummary {
        duration: media.duration_seconds,
        width: video.and_then(|v| v.width).unwrap_or(0),
        height: video.and_then(|v| v.height).unwrap_or(0),
        frame_rate: video.and_then(|v| v.frame_rate).unwrap_or(0.0),
        video_codec: video.map(|v| v.codec_name.clone()).filter(|name| !name.is_empty()),
        audio_codec: audio.map(|a| a.codec_name.clone()).filter(|name| !name.is_empty()),
        audio_sample_rate: audio.and_then(|a| a.sample_rate).unwrap_or(0),
        audio_channels: audio.and_then(|a| a.channels).unwrap_or(0),
        has_video: media.has_video(),
        has_audio: media.has_audio(),
    }
}

/// 上下文装配 + 命令行构建（`templates.md` §3.6 / §3.7）。
///
/// 返回 `(构建结果, 硬件解码参数, 本次是否启用了硬件解码)`。
pub(crate) fn build_attempt(
    task: &ConversionTask,
    template: &CommandTemplate,
    encoder: Option<EncoderProfile>,
    media: Option<&MediaInfo>,
    settings: &crate::models::AppSettings,
    disable_hardware_decode: bool,
) -> (BuiltCommand, String, bool) {
    let hardware_decode_args = build_hardware_decode_args(
        media,
        settings.prefer_hardware_decoding && !disable_hardware_decode,
    );
    let hardware_decode_enabled = !hardware_decode_args.is_empty();

    let context = CommandContext {
        input_path: task.input_path.clone(),
        output_path: task.output_path.clone(),
        parameters: merged_parameters(template, task.template_parameters.as_ref()),
        encoder,
        audio_encoder: resolve_audio_encoder(template),
        quality: None,
        media: media.map(media_summary),
        threads: 0,
        hardware_decode_args: hardware_decode_args.clone(),
        global_arguments: settings.global_ffmpeg_arguments.clone(),
    };

    (build(template, &context), hardware_decode_args, hardware_decode_enabled)
}

// ────────────────────────────────────────────────────────────────────────────
// 编码器解析（规格 §3.6.1 / templates.md §5.2）
// ────────────────────────────────────────────────────────────────────────────

/// 是否需要挑编码器、挑不到怎么办（规格 §3.6.1）。
///
/// * 模板自带编码器（没有 `{encoder}` 且 `hardware_mode = None`）→ `Ok(None)`，不做任何注入；
/// * `HardwareMode::Required` 且本机没有该族 → `Err(中文消息)`（绝不静默换厂商）。
pub(crate) async fn resolve_encoder(
    detector: &HardwareDetector,
    template: &CommandTemplate,
) -> Result<Option<EncoderProfile>, String> {
    let uses_placeholder = template.command_args.to_ascii_lowercase().contains("{encoder}");
    let needs_encoder =
        uses_placeholder || template.hardware_mode != HardwareAccelerationMode::None;
    if !needs_encoder {
        return Ok(None);
    }

    let codec = determine_target_codec(template);
    let required = template
        .required_encoder
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let strict = template.hardware_mode == HardwareAccelerationMode::Required;

    let Some(name) = detector.recommended_encoder_with(&codec, required, strict).await else {
        return Err(required_hardware_message(required));
    };

    // `Find(name) ?? SoftwareFallback(codec) ?? Find("libx264")`
    let Some(mut profile) = EncoderCatalog::find(&name)
        .or_else(|| EncoderCatalog::software_encoder_for(&codec))
        .or_else(|| EncoderCatalog::find("libx264"))
        .cloned()
    else {
        return Ok(None);
    };

    // 检测报告说该编码器只通过了最简参数试跑 → 丢掉质量/预设参数（规格 §3.6.1 末尾）
    let report = detector.detect(false).await;
    if let Some(detected) = report.find_encoder(&profile.name) {
        if !detected.verified_by_trial {
            profile.quality_args = String::new();
            profile.preset_args = String::new();
        }
    }
    Ok(Some(profile))
}

/// `Required` 模式下本机没有该硬件族时的中文消息（逐字来自规格 §3.6.1）。
pub(crate) fn required_hardware_message(required: Option<&str>) -> String {
    let family = match required {
        None => "指定的硬件编码器".to_string(),
        Some(value) => {
            let key = EncoderCatalog::find(value)
                .map(|profile| profile.family.clone())
                .unwrap_or_else(|| value.to_string());
            EncoderCatalog::family_display_name(&key)
        }
    };
    format!(
        "该模板要求使用 {family} 硬件编码，但本机不可用。\
         请改用「推荐」分类下会自动挑选编码器的模板，或在「设置 → 硬件加速能力」中确认本机支持的编码器。"
    )
}

// ────────────────────────────────────────────────────────────────────────────
// 回退判定（规格 §3.6.4 + 本项目有意差异 #3）
// ────────────────────────────────────────────────────────────────────────────

/// 是否应该回退到软件路径。
///
/// 规则（规格 §3.6.4 的 `ShouldFallbackToSoftware`）：
/// 1. 超时 → 不回退（超时不是硬件问题）；
/// 2. 本次尝试**没有产出任何数据** → 回退（基本可确定是硬件初始化失败）；
/// 3. 否则只在诊断命中关键字白名单时回退。
///
/// PORT NOTE（本项目有意差异 #3）：参考实现在判定前把 `ProducedAnyOutput` 清成了 `false`，
/// 于是这一步**永远**走「未产出任何数据」分支、关键字白名单形同虚设 —— 「跑到一半才失败的硬件编码
/// 任务也会被整条重跑一次」。这里按注释表达的意图实现：`produced_any_output` 由调用方在**每轮执行前
/// 重置、执行后取值**，因此「已经产出过数据」的失败必须命中关键字才回退。
pub(crate) fn should_fallback_to_software(outcome: &RunOutcome, produced_any_output: bool) -> bool {
    if outcome.timed_out {
        return false;
    }
    if !produced_any_output {
        return true;
    }
    let diagnostic = outcome.stderr_tail.to_ascii_lowercase();
    FALLBACK_KEYWORDS
        .iter()
        .any(|keyword| diagnostic.contains(&keyword.to_ascii_lowercase()))
}

/// 诊断首行（规格：首行、最长 300 字符）。
pub(crate) fn first_line(text: &str, max_chars: usize) -> String {
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            return truncate_chars(trimmed, max_chars);
        }
    }
    String::new()
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push('…');
    out
}

// ────────────────────────────────────────────────────────────────────────────
// 进度摘要行（规格 §3.7）
// ────────────────────────────────────────────────────────────────────────────

/// 供日志用的进度摘要行：`frame=…  bitrate=…  speed=…x  time=hh:mm:ss.ff`，
/// 缺失项省略，项之间**两个空格**（逐字来自规格 §3.7）。
pub(crate) fn progress_raw_output(tick: &ProgressTick) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(frame) = tick.frame {
        parts.push(format!("frame={frame}"));
    }
    if let Some(bitrate) = tick.bitrate.as_deref().filter(|value| !value.is_empty()) {
        parts.push(format!("bitrate={bitrate}"));
    }
    if let Some(speed) = tick.speed {
        parts.push(format!("speed={speed:.2}x"));
    }
    parts.push(format!("time={}", format_timecode(tick.out_time_seconds)));
    parts.join("  ")
}

/// `HH:MM:SS.ff`
pub(crate) fn format_timecode(seconds: f64) -> String {
    let total = if seconds.is_finite() && seconds > 0.0 { seconds } else { 0.0 };
    let hours = (total / 3600.0).floor() as u64;
    let minutes = ((total % 3600.0) / 60.0).floor() as u64;
    let secs = total % 60.0;
    format!("{hours:02}:{minutes:02}:{secs:05.2}")
}

// ────────────────────────────────────────────────────────────────────────────
// 日志截断（规格 §3.8）
// ────────────────────────────────────────────────────────────────────────────

/// 截断结果
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TruncatedLog {
    /// 截断后的文本
    pub text: String,
    /// 被丢掉的行数
    pub skipped: usize,
    /// 是否发生了截断
    pub truncated: bool,
}

/// 按**行数**上限截断日志：丢弃最早的行，最前面补一行
/// `…（已省略早期 N 行日志）`；`max_lines <= 0` 表示不截断。
///
/// PORT NOTE：参考实现是「保留最后 `maxLines` 行 + 标记行」，结果 txt 会变成
/// `maxLines + 1` 行（比上限多一行）。这里按规格「行数是上限行数」的字面语义
/// 保留最后 `maxLines - 1` 行再加标记行，保证结果**不超过**上限。
pub(crate) fn truncate_log(text: &str, max_lines: i32) -> TruncatedLog {
    if text.is_empty() {
        return TruncatedLog { text: String::new(), skipped: 0, truncated: false };
    }
    if max_lines <= 0 {
        return TruncatedLog { text: text.to_string(), skipped: 0, truncated: false };
    }

    let max_lines = max_lines as usize;
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.len() <= max_lines {
        return TruncatedLog { text: text.to_string(), skipped: 0, truncated: false };
    }

    let keep = max_lines.saturating_sub(1);
    let skipped = lines.len() - keep;
    let marker = format!("…（已省略早期 {skipped} 行日志）");
    if keep == 0 {
        return TruncatedLog { text: marker, skipped, truncated: true };
    }
    let kept = lines[lines.len() - keep..].join("\n");
    TruncatedLog { text: format!("{marker}\n{kept}"), skipped, truncated: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template(args: &str) -> CommandTemplate {
        CommandTemplate { command_args: args.to_string(), ..CommandTemplate::default() }
    }

    #[test]
    fn target_codec_follows_the_documented_priority() {
        // 1) 显式声明优先
        let mut t = template("-c:v libx265");
        t.target_codec = "avc".to_string();
        assert_eq!(determine_target_codec(&t), "h264");

        // 2) required_encoder（具体名）
        let mut t = template("-c:v {encoder}");
        t.required_encoder = Some("hevc_nvenc".to_string());
        assert_eq!(determine_target_codec(&t), "hevc");

        // 2) required_encoder（族名 → 该族第一个编码器）
        let mut t = template("-c:v {encoder}");
        t.required_encoder = Some("nvenc".to_string());
        assert_eq!(determine_target_codec(&t), "h264");

        // 3) 输出扩展名归一
        let mut t = template("-c:v {encoder}");
        t.output_extension = "webm".to_string();
        assert_eq!(determine_target_codec(&t), "vp9");

        // 4) 命令里出现的编码器名（libx265 先于 libx264）
        let t = template("-c:v libx265 -c:v libx264");
        assert_eq!(determine_target_codec(&t), "hevc");
        let t = template("-c:v libvpx-vp9");
        assert_eq!(determine_target_codec(&t), "vp9");

        // 5) 扩展名兜底
        let mut t = template("-c:v copy");
        t.output_extension = "mkv".to_string();
        assert_eq!(determine_target_codec(&t), "h264");
        let mut t = template("-c:v copy");
        t.output_extension = "mp4".to_string();
        assert_eq!(determine_target_codec(&t), "h264");
    }

    #[test]
    fn audio_encoder_prefers_the_template_then_the_extension() {
        assert_eq!(resolve_audio_encoder(&template("-c:a libopus")), "libopus");
        assert_eq!(resolve_audio_encoder(&template("-c:a libmp3lame")), "libmp3lame");
        assert_eq!(resolve_audio_encoder(&template("-c:a flac")), "flac");
        assert_eq!(resolve_audio_encoder(&template("-c:a aac")), "aac");

        let mut webm = template("-c:v libvpx-vp9");
        webm.output_extension = "webm".to_string();
        assert_eq!(resolve_audio_encoder(&webm), "libopus");
        let mut mp3 = template("-vn");
        mp3.output_extension = "mp3".to_string();
        assert_eq!(resolve_audio_encoder(&mp3), "libmp3lame");
        let mut plain = template("-c:v libx264");
        plain.output_extension = "mp4".to_string();
        assert_eq!(resolve_audio_encoder(&plain), "aac");
    }

    #[test]
    fn hardware_decode_args_require_a_supported_video_codec() {
        assert!(build_hardware_decode_args(None, true).is_empty());
        // 关闭时不注入
        assert!(build_hardware_decode_args(None, false).is_empty());

        let mut media = MediaInfo {
            path: String::new(),
            file_name: String::new(),
            size_bytes: 0,
            duration_seconds: 1.0,
            format_name: String::new(),
            format_long_name: String::new(),
            bit_rate: 0,
            video: None,
            audio: None,
            streams: Vec::new(),
        };
        // 没有视频流 → 空
        assert!(build_hardware_decode_args(Some(&media), true).is_empty());

        media.video = Some(crate::ffmpeg::service::MediaStream {
            codec_type: "video".to_string(),
            codec_name: "h264".to_string(),
            ..Default::default()
        });
        assert_eq!(build_hardware_decode_args(Some(&media), true), format!("-hwaccel {}", platform_hwaccel()));

        media.video = Some(crate::ffmpeg::service::MediaStream {
            codec_type: "video".to_string(),
            codec_name: "theora".to_string(),
            ..Default::default()
        });
        assert!(build_hardware_decode_args(Some(&media), true).is_empty());
    }

    #[test]
    fn fallback_requires_no_output_or_a_known_hardware_error() {
        let clean_failure = RunOutcome {
            exit_code: Some(1),
            success: false,
            cancelled: false,
            timed_out: false,
            elapsed_ms: 10,
            stderr_tail: "some unrelated error".to_string(),
            log_lines: Vec::new(),
        };
        // 一帧都没产出 → 回退
        assert!(should_fallback_to_software(&clean_failure, false));
        // 已产出数据且诊断无关 → 不回退（这正是差异 #3 修正的地方）
        assert!(!should_fallback_to_software(&clean_failure, true));

        let hardware_failure = RunOutcome {
            stderr_tail: "Cannot load libcuda.so.1".to_string(),
            ..clean_failure.clone()
        };
        assert!(should_fallback_to_software(&hardware_failure, true));
        assert!(should_fallback_to_software(&hardware_failure, false));

        let timed_out = RunOutcome { timed_out: true, ..clean_failure.clone() };
        assert!(!should_fallback_to_software(&timed_out, false));

        let cancelled = RunOutcome { cancelled: true, ..clean_failure.clone() };
        // 取消本身不是硬件问题：没有产出时仍会「回退」——调用方必须先排除取消（见 engine）
        assert!(should_fallback_to_software(&cancelled, false));
    }

    #[test]
    fn progress_line_matches_the_documented_shape() {
        let tick = ProgressTick {
            frame: Some(42),
            fps: Some(25.0),
            bitrate: Some("1234.5kbits/s".to_string()),
            total_size: Some(1024),
            out_time_seconds: 3.5,
            speed: Some(1.25),
            percent: 35.0,
            progress: Some("continue".to_string()),
        };
        assert_eq!(
            progress_raw_output(&tick),
            "frame=42  bitrate=1234.5kbits/s  speed=1.25x  time=00:00:03.50"
        );

        let sparse = ProgressTick {
            frame: None,
            fps: None,
            bitrate: None,
            total_size: None,
            out_time_seconds: 0.0,
            speed: None,
            percent: 0.0,
            progress: Some("continue".to_string()),
        };
        assert_eq!(progress_raw_output(&sparse), "time=00:00:00.00");
    }

    #[test]
    fn log_truncation_drops_the_oldest_lines_and_stays_within_the_cap() {
        let text = (1..=20).map(|index| format!("行{index}")).collect::<Vec<_>>().join("\n");

        let untouched = truncate_log(&text, 50);
        assert_eq!(untouched.text, text);
        assert!(!untouched.truncated);
        assert_eq!(untouched.skipped, 0);

        let truncated = truncate_log(&text, 10);
        assert!(truncated.truncated);
        assert_eq!(truncated.skipped, 11);
        assert!(truncated.text.starts_with("…（已省略早期 11 行日志）"));
        assert_eq!(truncated.text.lines().count(), 10);
        assert!(truncated.text.ends_with("行20"));

        // max_lines <= 0 → 不截断
        let unlimited = truncate_log(&text, 0);
        assert_eq!(unlimited.text, text);
        assert!(!unlimited.truncated);

        assert!(truncate_log("", 10).text.is_empty());
    }

    #[test]
    fn first_line_is_trimmed_and_truncated() {
        assert_eq!(first_line("\n  hello  \nworld", 300), "hello");
        assert_eq!(first_line("", 10), "");
        assert_eq!(first_line("x".repeat(400).as_str(), 300).chars().count(), 301);
    }

    #[test]
    fn required_hardware_message_is_verbatim() {
        let message = required_hardware_message(Some("nvenc"));
        assert!(message.starts_with("该模板要求使用 NVIDIA NVENC 硬件编码，但本机不可用。"));
        let generic = required_hardware_message(None);
        assert!(generic.starts_with("该模板要求使用 指定的硬件编码器 硬件编码，但本机不可用。"));
        let unknown = required_hardware_message(Some("weird_family"));
        assert!(unknown.starts_with("该模板要求使用 weird_family 硬件编码，但本机不可用。"));
    }

    #[test]
    fn merged_parameters_let_the_task_win() {
        let mut t = template("-crf {crf}");
        t.parameters = vec![
            crate::models::TemplateParameter {
                name: "crf".to_string(),
                default_value: "28".to_string(),
                ..Default::default()
            },
            crate::models::TemplateParameter {
                name: " ".to_string(),
                default_value: "ignored".to_string(),
                ..Default::default()
            },
        ];
        let defaults = default_parameter_values(&t);
        assert_eq!(defaults.get("crf").map(String::as_str), Some("28"));
        assert_eq!(defaults.len(), 1);

        let mut overrides = HashMap::new();
        overrides.insert("crf".to_string(), "18".to_string());
        let merged = merged_parameters(&t, Some(&overrides));
        assert_eq!(merged.get("crf").map(String::as_str), Some("18"));
    }
}
