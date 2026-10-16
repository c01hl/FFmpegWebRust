//! 任务预览（`POST /api/tasks/preview`）。
//!
//! 对应参考实现 `Components/Pages/Convert.razor` 的 `RebuildPreviewAsync`（`ui-spec.md` §5.1）。
//! 这是「前端不做任何本地推断」原则的核心端点：
//!
//! 1. 探测媒体信息（失败不算错误，`media = null`）；
//! 2. 加载模板（缺失是硬错误）；
//! 3. 算出「请求的输出路径」（命名模板 / 命名规则 + 扩展名）；
//! 4. 调 `resolve_output_path` 做冲突解析（含「输出 == 输入」安全检查）；
//! 5. 挑编码器（复用 `plan::resolve_encoder`，与队列同一套四级优先级）；
//! 6. 组装 `CommandContext`（复用 `plan` 的装配规则）并构建完整命令
//!    （含 `-hide_banner -y|-n -progress pipe:2` 前缀）；
//! 7. 汇总告警、模板校验问题、阻塞错误。
//!
//! 逐字文案见 `docs/api-contract.md` §1.5、`docs/reference/ui-spec.md` §4.2/§5.1、
//! `docs/reference/backend-data.md` §3.6/§4.1。

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use super::plan;
use crate::command::{self, CommandContext};
use crate::ffmpeg::encoder_catalog::{EncoderProfile, EncoderType};
use crate::ffmpeg::hardware::HardwareDetector;
use crate::ffmpeg::service::{FFmpegService, MediaInfo, MediaStream};
use crate::files::{
    format_file_name, generate_output_path, is_output_path_writable, resolve_output_path,
    sanitize_file_name,
};
use crate::models::enums::{FileExistsAction, OutputNamingRule};
use crate::models::{AppSettings, CommandTemplate, TemplateIssue};
use crate::paths;
use crate::services::SettingsService;
use crate::templates::TemplateService;

// ────────────────────────────────────────────────────────────────────────────
// 逐字文案（改动前先改 api-contract / ui-spec）
// ────────────────────────────────────────────────────────────────────────────

/// `ui-spec.md` §4.2：设置 = 跳过
pub const CONFLICT_NOTICE_SKIP: &str = "输出文件已存在：按当前设置将跳过本次转换，不会覆盖原文件。";
/// `ui-spec.md` §4.2：设置 = 重命名（`Ask` 在执行层等价于重命名）
pub const CONFLICT_NOTICE_RENAME: &str = "输出文件已存在，将自动改名后保存，避免覆盖原文件。";
/// `ui-spec.md` §4.2：设置 = 覆盖
pub const CONFLICT_NOTICE_OVERWRITE: &str = "输出文件已存在，将按设置覆盖。";
/// `ui-spec.md` §4.2 / §5.1：输出路径 == 输入文件（强制改名）
pub const PATHS_EQUAL_WARNING: &str = "输出路径与输入文件相同：已自动改名，避免把源文件覆盖掉。\
（把输出目录换成别的目录、或让输出文件名带上后缀，可以避免改名。）";
/// `api-contract.md` §0：模板缺失的错误消息（API 层把它包成 `TEMPLATE_NOT_FOUND`）
pub const TEMPLATE_MISSING_MESSAGE: &str = "模板不存在或已被删除";
/// `ui-spec.md` §4.2：输出目录无法确定（输出目录为空时）
pub const OUTPUT_PATH_UNDETERMINED_MESSAGE: &str = "无法确定输出路径，请检查输出目录设置。";

/// `backend-data.md` §3.6.1：严格模式（`Required`）缺硬件时的逐字错误。
///
/// 直接复用队列的 `plan::required_hardware_message`，保证「预览说不可运行」与
/// 「执行时真的失败」用的是同一条消息。
pub fn strict_hardware_error(required: Option<&str>) -> String {
    plan::required_hardware_message(required)
}

/// `ui-spec.md` §4.2：输出目录不可写
pub fn output_directory_not_writable_error(directory: &str) -> String {
    format!("输出目录不可写：{directory}。请换一个目录或检查文件权限。")
}

/// `serde` 默认值：`allowRename` 省略时为 `true`
fn default_true() -> bool {
    true
}

// ────────────────────────────────────────────────────────────────────────────
// 请求 / 响应
// ────────────────────────────────────────────────────────────────────────────

/// `POST /api/tasks/preview` 请求体（逐字段见 `api-contract.md` §1.5）
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRequest {
    pub input_path: String,
    pub output_directory: String,
    /// 可空：输出文件名模板（支持 `{filename}` 等占位符）；空 → 走设置里的命名规则
    #[serde(default)]
    pub output_file_name_template: Option<String>,
    /// 可空：输出扩展名；空 → 取模板的 `outputExtension`，再空 → `mp4`
    #[serde(default)]
    pub output_extension: Option<String>,
    pub template_id: String,
    #[serde(default)]
    pub parameter_values: HashMap<String, String>,
    /// 界面上的「覆盖已有文件」按钮：true → 强制按覆盖策略解析
    #[serde(default)]
    pub overwrite_existing: bool,
    /// 是否允许自动改名（false 时把「重命名 / 询问」策略降级成覆盖）
    #[serde(default = "default_true")]
    pub allow_rename: bool,
}

/// 预览里解析出的编码器画像（`EncoderProfileDto`）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncoderProfileDto {
    pub name: String,
    pub display_name: String,
    /// 枚举名（`Software` / `Nvenc` / `VideoToolbox` …）
    #[serde(rename = "type")]
    pub encoder_type: EncoderType,
    pub family: String,
    pub codec: String,
    /// 含 `{q}` 占位的原始质量参数（契约 §1.5 示例即为 `-q:v {q}`）
    pub quality_args: String,
    pub quality_hint: String,
    pub is_hardware: bool,
}

impl EncoderProfileDto {
    /// 由画像构造（`quality_args` 原样保留 `{q}`，界面负责展示）
    pub fn from_profile(profile: &EncoderProfile) -> Self {
        Self {
            name: profile.name.clone(),
            display_name: profile.display_name.clone(),
            encoder_type: profile.encoder_type,
            family: profile.family.clone(),
            codec: profile.codec.clone(),
            quality_args: profile.quality_args.clone(),
            quality_hint: profile.quality_hint.clone(),
            is_hardware: profile.is_hardware(),
        }
    }
}

/// 单条流信息（`MediaStreamInfoDto`）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaStreamInfoDto {
    pub index: i32,
    pub codec_type: String,
    pub codec_name: Option<String>,
    pub codec_long_name: Option<String>,
    pub width: i32,
    pub height: i32,
    pub frame_rate: f64,
    pub duration: f64,
    pub channels: i32,
    pub sample_rate: i32,
    pub bit_rate: i64,
    pub rotation: i32,
    pub pixel_format: Option<String>,
}

impl MediaStreamInfoDto {
    fn from_stream(stream: &MediaStream) -> Self {
        Self {
            index: stream.index,
            codec_type: stream.codec_type.clone(),
            codec_name: none_if_blank(&stream.codec_name),
            codec_long_name: none_if_blank(&stream.codec_long_name),
            width: stream.width.unwrap_or(0),
            height: stream.height.unwrap_or(0),
            frame_rate: stream.frame_rate.unwrap_or(0.0),
            duration: stream.duration.unwrap_or(0.0),
            channels: stream.channels.unwrap_or(0),
            sample_rate: stream.sample_rate.unwrap_or(0),
            bit_rate: stream.bit_rate.unwrap_or(0),
            rotation: stream.rotation,
            pixel_format: stream.pixel_format.clone(),
        }
    }
}

/// 媒体文件信息（`MediaInfoDto`）。
///
/// PORT NOTE：`ffmpeg::service::MediaInfo` 的字段名是 Rust 风格（`file_name`/`duration_seconds`），
/// 而契约（`api-contract.md` §3、`web/src/api/types.ts`）要求 camelCase 且**语义不同**的
/// `MediaInfoDto`（`filePath`/`duration`/`format`/`formatDisplay`…）。
/// 因此这里做一次显式投影，而不是直接嵌 `MediaInfo`（否则 JSON 键名不符合契约）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfoDto {
    pub file_path: String,
    pub duration: f64,
    pub format: String,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub width: i32,
    pub height: i32,
    pub frame_rate: f64,
    pub file_size: u64,
    pub format_display: String,
    pub bit_rate: i64,
    pub audio_channels: i32,
    pub audio_sample_rate: i32,
    pub rotation: i32,
    pub pixel_format: Option<String>,
    pub has_video: bool,
    pub has_audio: bool,
    pub subtitle_stream_count: usize,
    pub streams: Vec<MediaStreamInfoDto>,
}

impl MediaInfoDto {
    /// `ffprobe` 结果 → 契约里的 `MediaInfoDto`
    pub fn from_media(media: &MediaInfo) -> Self {
        Self {
            file_path: media.path.clone(),
            duration: media.duration_seconds,
            format: media.format_name.clone(),
            video_codec: media.video.as_ref().and_then(|video| none_if_blank(&video.codec_name)),
            audio_codec: media.audio.as_ref().and_then(|audio| none_if_blank(&audio.codec_name)),
            width: media.video.as_ref().and_then(|video| video.width).unwrap_or(0),
            height: media.video.as_ref().and_then(|video| video.height).unwrap_or(0),
            frame_rate: media.video.as_ref().and_then(|video| video.frame_rate).unwrap_or(0.0),
            file_size: media.size_bytes,
            format_display: media.format_display(),
            bit_rate: media.bit_rate,
            audio_channels: media.audio.as_ref().and_then(|audio| audio.channels).unwrap_or(0),
            audio_sample_rate: media.audio.as_ref().and_then(|audio| audio.sample_rate).unwrap_or(0),
            rotation: media.rotation(),
            pixel_format: media.video.as_ref().and_then(|video| video.pixel_format.clone()),
            has_video: media.has_video(),
            has_audio: media.has_audio(),
            subtitle_stream_count: media.subtitle_stream_count(),
            streams: media.streams.iter().map(MediaStreamInfoDto::from_stream).collect(),
        }
    }
}

/// `POST /api/tasks/preview` 响应（逐字段见 `api-contract.md` §1.5）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewResponse {
    /// 媒体信息；探测失败时为 `null`（不是错误）
    pub media: Option<MediaInfoDto>,
    /// 按命名规则算出的「期望输出路径」
    pub requested_output_path: String,
    /// 冲突解析后的最终输出路径
    pub resolved_output_path: String,
    /// 相对请求路径是否发生了自动改名
    pub renamed: bool,
    /// 是否应当跳过本次转换（设置里的策略 = 跳过）
    pub skip: bool,
    /// 决定 ffmpeg 加 `-y`(true) / `-n`(false)
    pub overwrite: bool,
    /// 冲突提示（黄条 + 两个按钮）；`null` = 没有冲突或已强制覆盖
    pub conflict_notice: Option<String>,
    /// 「输出路径 == 输入文件」的警告；`null` = 没有命中
    pub paths_equal_warning: Option<String>,
    pub output_directory_writable: bool,
    pub encoder: Option<EncoderProfileDto>,
    /// 完整命令（不含 `ffmpeg ` 前缀；含 `-hide_banner -y|-n -progress pipe:2`）
    pub command_preview: String,
    /// 命令构建告警（逐字来自 `BuiltCommand.warnings`）
    pub warnings: Vec<String>,
    /// 阻塞错误（严格模式缺硬件 / 输出目录不可写）；`null` = 可执行
    pub blocking_error: Option<String>,
    pub template_runnable: bool,
    pub uses_hardware_acceleration: bool,
    pub template_name: String,
    /// 模板校验问题（`TemplateService::validate` 的结果）
    pub issues: Vec<TemplateIssue>,
}

// ────────────────────────────────────────────────────────────────────────────
// 预览主流程
// ────────────────────────────────────────────────────────────────────────────

/// 构建预览（`TaskService::preview` 的实现体）。
pub async fn build_preview(
    settings: &SettingsService,
    templates: &TemplateService,
    ffmpeg: &FFmpegService,
    hardware: &HardwareDetector,
    request: &PreviewRequest,
) -> Result<PreviewResponse> {
    let app_settings = settings.get().await;
    let input_path = paths::to_absolute(&request.input_path).to_string_lossy().to_string();

    // 1) 模板：缺失是硬错误（API 层映射成 TEMPLATE_NOT_FOUND / 404）
    let template = load_template(templates, &request.template_id).await?;

    // 2) 媒体探测：失败不是错误（文件不存在、格式不支持等都只是 `media = null`）
    let media = ffmpeg.probe_media(&input_path).await.ok();

    // 3) 扩展名与「请求的输出路径」
    let extension = resolve_extension(request.output_extension.as_deref(), &template);
    let requested_path = requested_output_path(request, &input_path, &extension, &app_settings)?;
    let requested_absolute = paths::to_absolute(&requested_path).to_string_lossy().to_string();

    // 4) 冲突解析（`overwrite_existing` 等价于界面上的「覆盖已有文件」）
    let action = resolve_action(request.overwrite_existing, request.allow_rename, app_settings.file_exists_action);
    let resolution = resolve_output_path(&requested_path, action, &input_path);

    // 「输出 == 输入」优先于一切策略（§4.1 第 2 条），命中时执行层会强制改名
    let paths_equal = paths::paths_equal(&requested_absolute, &input_path);
    let requested_exists = Path::new(&requested_absolute).is_file();

    let paths_equal_warning = paths_equal.then(|| PATHS_EQUAL_WARNING.to_string());
    let conflict_notice = if paths_equal || request.overwrite_existing {
        // 输出==输入：提示由 `paths_equal_warning` 承载；
        // 强制覆盖：§5.1 第 3 步明确「未强制覆盖时才生成冲突提示」
        None
    } else if resolution.skip {
        Some(CONFLICT_NOTICE_SKIP.to_string())
    } else if resolution.renamed {
        Some(CONFLICT_NOTICE_RENAME.to_string())
    } else if requested_exists {
        Some(CONFLICT_NOTICE_OVERWRITE.to_string())
    } else {
        None
    };

    // 5) 编码器：复用队列的解析（同一套四级优先级；严格模式缺硬件 → 阻塞错误）
    let (encoder, mut blocking_error) = match plan::resolve_encoder(hardware, &template).await {
        Ok(profile) => (profile, None),
        Err(message) => (None, Some(message)),
    };

    // 6) 完整命令（上下文装配与队列完全一致，见 backend-data.md §3.7）
    let mut context = CommandContext::new(&input_path, &resolution.output_path);
    context.parameters = plan::merged_parameters(&template, Some(&request.parameter_values));
    context.encoder = encoder.clone();
    context.audio_encoder = plan::resolve_audio_encoder(&template);
    context.media = media.as_ref().map(plan::media_summary);
    context.hardware_decode_args =
        plan::build_hardware_decode_args(media.as_ref(), app_settings.prefer_hardware_decoding);
    context.global_arguments = app_settings.global_ffmpeg_arguments.clone();

    let built = command::build(&template, &context);
    let mut arguments: Vec<String> = vec![
        "-hide_banner".to_string(),
        if resolution.overwrite { "-y" } else { "-n" }.to_string(),
        "-progress".to_string(),
        "pipe:2".to_string(),
    ];
    arguments.extend(built.arguments.iter().cloned());
    let command_preview = command::render_command_line(&arguments);

    // 7) 可执行性与阻塞错误
    let output_directory_writable = is_output_path_writable(&resolution.output_path);
    if !output_directory_writable && blocking_error.is_none() {
        blocking_error = Some(output_directory_not_writable_error(&output_directory_of(
            &resolution.output_path,
            &request.output_directory,
        )));
    }

    // 8) 模板校验（「本机缺少必需滤镜」由 filters 传入，见 templates.md §7.1）
    let hardware_report = hardware.detect(false).await;
    let issues = templates.validate(&template, Some(hardware_report.supported_filters.as_slice()));

    let uses_hardware_acceleration = encoder.as_ref().is_some_and(|profile| profile.is_hardware());

    Ok(PreviewResponse {
        media: media.as_ref().map(MediaInfoDto::from_media),
        requested_output_path: requested_path,
        resolved_output_path: resolution.output_path,
        renamed: resolution.renamed,
        skip: resolution.skip,
        overwrite: resolution.overwrite,
        conflict_notice,
        paths_equal_warning,
        output_directory_writable,
        encoder: encoder.as_ref().map(EncoderProfileDto::from_profile),
        command_preview,
        warnings: built.warnings,
        template_runnable: blocking_error.is_none(),
        blocking_error,
        uses_hardware_acceleration,
        template_name: template.name.clone(),
        issues,
    })
}

/// 请求的输出路径（§4.8 单文件路径）。
fn requested_output_path(
    request: &PreviewRequest,
    input_path: &str,
    extension: &str,
    settings: &AppSettings,
) -> Result<String> {
    if request.output_directory.trim().is_empty() {
        // 原实现用 `Path.Combine` 会把空的输出目录落到进程 CWD（隐患，见 §4.8），这里直接报错
        return Err(anyhow!(OUTPUT_PATH_UNDETERMINED_MESSAGE));
    }

    let file_name_template = request.output_file_name_template.as_deref().unwrap_or("").trim();
    if file_name_template.is_empty() {
        // 空模板 → 输入文件名 +（命名规则 = 添加后缀时）设置里的后缀
        let suffix = if settings.output_naming == OutputNamingRule::Suffix {
            Some(settings.output_suffix.as_str())
        } else {
            None
        };
        let path = generate_output_path(input_path, &request.output_directory, extension, suffix);
        if path.trim().is_empty() {
            return Err(anyhow!(OUTPUT_PATH_UNDETERMINED_MESSAGE));
        }
        return Ok(path);
    }

    // 命名模板：先展开占位符，再清洗（顺序不能反，否则 `/` 能逃出输出目录，§4.7）
    let name = sanitize_file_name(&format_file_name(file_name_template, input_path, extension));
    Ok(Path::new(&request.output_directory)
        .join(format!("{name}.{extension}"))
        .to_string_lossy()
        .to_string())
}

/// 输出扩展名：请求 → 模板 `outputExtension` → `mp4`（去前导点、小写）
fn resolve_extension(requested: Option<&str>, template: &CommandTemplate) -> String {
    for candidate in [requested.unwrap_or(""), template.output_extension.as_str()] {
        let trimmed = candidate.trim().trim_start_matches('.').to_lowercase();
        if !trimmed.is_empty() {
            return trimmed;
        }
    }
    "mp4".to_string()
}

/// 冲突解析用的策略：强制覆盖 → `Overwrite`；不允许改名 → 把「重命名 / 询问」降级成 `Overwrite`
fn resolve_action(overwrite_existing: bool, allow_rename: bool, configured: FileExistsAction) -> FileExistsAction {
    if overwrite_existing {
        return FileExistsAction::Overwrite;
    }
    if allow_rename {
        return configured;
    }
    // PORT NOTE：`allowRename = false` 在参考实现里没有对应字段（是 Rust 侧为界面按钮加的开关）。
    // 语义取最保守的解释：调用方明确不要「自动改名」，于是把会改名的策略按「覆盖」处理
    // （「输出 == 输入」的强制改名不受影响 —— 那是硬安全规则，优先于一切策略）。
    match configured {
        FileExistsAction::Rename | FileExistsAction::Ask => FileExistsAction::Overwrite,
        other => other,
    }
}

/// 输出目录的展示路径（不可写错误里用）
fn output_directory_of(output_path: &str, fallback: &str) -> String {
    Path::new(output_path)
        .parent()
        .map(|parent| parent.to_string_lossy().to_string())
        .filter(|parent| !parent.trim().is_empty())
        .unwrap_or_else(|| fallback.trim().to_string())
}

/// 空白字符串 → `None`（`videoCodec` 之类的字段需要「没有值」而不是空串）
fn none_if_blank(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 加载模板；缺失 → 契约里的中文错误
async fn load_template(templates: &TemplateService, id: &str) -> Result<CommandTemplate> {
    match templates.get(id).await {
        Ok(Some(template)) => Ok(template),
        Ok(None) => Err(anyhow!(TEMPLATE_MISSING_MESSAGE)),
        Err(error) => Err(error.context(format!("读取模板失败：{id}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template() -> CommandTemplate {
        CommandTemplate {
            command_args: "-i {input} -c:v {encoder} {quality_args} -c:a aac {output}".to_string(),
            output_extension: "mp4".to_string(),
            ..CommandTemplate::default()
        }
    }

    #[test]
    fn extension_prefers_request_then_template_then_mp4() {
        let template = template();
        assert_eq!(resolve_extension(Some("MKV"), &template), "mkv");
        assert_eq!(resolve_extension(Some(".webm"), &template), "webm");
        assert_eq!(resolve_extension(None, &template), "mp4");

        let mut blank = template.clone();
        blank.output_extension = "  ".to_string();
        assert_eq!(resolve_extension(Some(""), &blank), "mp4");
    }

    #[test]
    fn action_resolution_covers_forced_overwrite_and_no_rename() {
        assert_eq!(resolve_action(true, true, FileExistsAction::Ask), FileExistsAction::Overwrite);
        assert_eq!(resolve_action(false, true, FileExistsAction::Ask), FileExistsAction::Ask);
        assert_eq!(resolve_action(false, false, FileExistsAction::Ask), FileExistsAction::Overwrite);
        assert_eq!(resolve_action(false, false, FileExistsAction::Rename), FileExistsAction::Overwrite);
        assert_eq!(resolve_action(false, false, FileExistsAction::Skip), FileExistsAction::Skip);
    }

    #[test]
    fn strict_hardware_error_is_verbatim() {
        let message = strict_hardware_error(Some("nvenc"));
        assert!(message.starts_with("该模板要求使用 NVIDIA NVENC 硬件编码，但本机不可用。"), "{message}");
        assert!(message.ends_with("中确认本机支持的编码器。"), "{message}");

        // 没写 `requiredEncoder` 时用字面量兜底
        assert!(strict_hardware_error(None).starts_with("该模板要求使用 指定的硬件编码器 硬件编码"));

        assert_eq!(
            output_directory_not_writable_error("/out"),
            "输出目录不可写：/out。请换一个目录或检查文件权限。"
        );
    }

    #[test]
    fn notices_are_verbatim() {
        assert_eq!(CONFLICT_NOTICE_SKIP, "输出文件已存在：按当前设置将跳过本次转换，不会覆盖原文件。");
        assert_eq!(CONFLICT_NOTICE_RENAME, "输出文件已存在，将自动改名后保存，避免覆盖原文件。");
        assert_eq!(CONFLICT_NOTICE_OVERWRITE, "输出文件已存在，将按设置覆盖。");
        assert_eq!(
            PATHS_EQUAL_WARNING,
            "输出路径与输入文件相同：已自动改名，避免把源文件覆盖掉。\
（把输出目录换成别的目录、或让输出文件名带上后缀，可以避免改名。）"
        );
        assert_eq!(TEMPLATE_MISSING_MESSAGE, "模板不存在或已被删除");
        assert_eq!(OUTPUT_PATH_UNDETERMINED_MESSAGE, "无法确定输出路径，请检查输出目录设置。");
    }

    #[test]
    fn output_directory_of_prefers_parent_directory() {
        assert_eq!(output_directory_of("/out/a.mp4", "/fallback"), "/out");
        assert_eq!(output_directory_of("relative.mp4", "/fallback"), "/fallback");
    }

    /// 契约要求 camelCase 键名（`web/src/api/types.ts` 的 `TaskPreviewDto`）
    #[test]
    fn response_serialises_with_contract_keys() {
        let response = PreviewResponse {
            media: None,
            requested_output_path: "/out/a.mp4".to_string(),
            resolved_output_path: "/out/a (1).mp4".to_string(),
            renamed: true,
            skip: false,
            overwrite: true,
            conflict_notice: Some(CONFLICT_NOTICE_RENAME.to_string()),
            paths_equal_warning: None,
            output_directory_writable: true,
            encoder: None,
            command_preview: "-hide_banner -y".to_string(),
            warnings: vec![],
            blocking_error: None,
            template_runnable: true,
            uses_hardware_acceleration: false,
            template_name: "模板".to_string(),
            issues: vec![],
        };
        let json = serde_json::to_value(&response).expect("序列化预览响应");
        for key in [
            "media",
            "requestedOutputPath",
            "resolvedOutputPath",
            "renamed",
            "skip",
            "overwrite",
            "conflictNotice",
            "pathsEqualWarning",
            "outputDirectoryWritable",
            "encoder",
            "commandPreview",
            "warnings",
            "blockingError",
            "templateRunnable",
            "usesHardwareAcceleration",
            "templateName",
            "issues",
        ] {
            assert!(json.get(key).is_some(), "缺少契约字段 {key}：{json}");
        }
        assert_eq!(json["conflictNotice"], CONFLICT_NOTICE_RENAME);
        assert!(json["pathsEqualWarning"].is_null());
    }

    /// 媒体 DTO 的字段名与派生值（`filePath` / `duration` / `formatDisplay` …）
    #[test]
    fn media_dto_projects_media_info() {
        let video = MediaStream {
            index: 0,
            codec_type: "video".to_string(),
            codec_name: "h264".to_string(),
            width: Some(1920),
            height: Some(1080),
            frame_rate: Some(29.97),
            pixel_format: Some("yuv420p".to_string()),
            rotation: 90,
            ..MediaStream::default()
        };
        let audio = MediaStream {
            index: 1,
            codec_type: "audio".to_string(),
            codec_name: "aac".to_string(),
            channels: Some(2),
            sample_rate: Some(48000),
            ..MediaStream::default()
        };
        let media = MediaInfo {
            path: "/videos/a.mp4".to_string(),
            file_name: "a.mp4".to_string(),
            size_bytes: 2048,
            duration_seconds: 12.5,
            format_name: "mov,mp4,m4a,3gp,3g2,mj2".to_string(),
            format_long_name: "QuickTime / MOV".to_string(),
            bit_rate: 1_000_000,
            video: Some(video),
            audio: Some(audio.clone()),
            streams: vec![audio],
        };

        let dto = MediaInfoDto::from_media(&media);
        let json = serde_json::to_value(&dto).expect("序列化媒体 DTO");
        assert_eq!(json["filePath"], "/videos/a.mp4");
        assert_eq!(json["duration"], 12.5);
        assert_eq!(json["format"], "mov,mp4,m4a,3gp,3g2,mj2");
        assert_eq!(json["formatDisplay"], "MP4 / MOV");
        assert_eq!(json["videoCodec"], "h264");
        assert_eq!(json["width"], 1920);
        assert_eq!(json["height"], 1080);
        assert_eq!(json["fileSize"], 2048);
        assert_eq!(json["rotation"], 90);
        assert_eq!(json["hasVideo"], true);
        assert_eq!(json["hasAudio"], true);
        assert_eq!(json["subtitleStreamCount"], 0);
        assert_eq!(json["streams"].as_array().map(Vec::len), Some(1));
        assert_eq!(json["streams"][0]["codecType"], "audio");
    }
}
