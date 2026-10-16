//! 内置模板预设目录（51 个，**逐字**移植自参考实现 `Services/SystemTemplatePresets.cs`）。
//!
//! 规格依据：`docs/reference/templates.md` §11（完整目录，逐字）、§12（生成器辅助函数，逐字）。
//!
//! 移植要点：
//! - `command_args` 与全部中文文案**逐字**照搬，不做任何「顺手修正」：包括
//!   `转换为 MP4（H.264 软件编码 x264)` 这类混用半角括号的写法、S33 的 `transpose=1` 死参数、
//!   S46 故意把参数命名为 `seek`（`{time}` 是内置占位符）等已知缺陷（见 §13.2）。
//! - 共享参数对象（`qualityAuto` / `audioBitrate` / `hardQuality`）在参考实现里是**同一个实例**
//!   被塞进多个模板；Rust 版每次调用都新建，天然按值深拷贝（§11.0）。
//! - 10 个厂商硬件模板由 [`add_hardware_family`] 生成、4 个缩放模板由 [`add_scale_template`] 生成，
//!   与参考实现的 `AddHardwareFamily` / `AddScaleTemplate` 等价，命令串逐字一致。
//! - `sort_order` 等于登记序号（0..=50），等价于 C# 的 `order++`。注意 **S51（音频 · 直接复制音轨）
//!   登记在最后**，所以它 `sort_order = 50` 但分类是「音频处理」（声明顺序 ≠ 展示顺序，§11 S51）。
//! - `type` 恒为 [`TemplateType::System`]，`preset_version` 恒为 [`PRESET_VERSION`]。
//!
//! # 稳定 key（本移植的额外设计，参考实现没有）
//!
//! 参考实现没有稳定模板 id：每条 `Id` 都是随机 `ObjectId`，同步靠 `Name` 匹配，
//! 因此改名会产生重复条目、删除会残留陈旧模板（§10.3）。本移植为每条预设定义**稳定 key**，
//! 并派生确定性 id：
//!
//! ```text
//! system_template_id(key) = SHA-256(key) 前 12 字节的小写十六进制（24 个字符）
//! ```
//!
//! 24 位小写十六进制与参考实现的 `ObjectId` 字符串格式完全一致，前端/任务表引用的 id 形状不变。
//! 同步算法用 key（[`stable_key`]）定位同一条预设，`name` 仅作展示文本，可随时改文案。
//!
//! key 命名规则：小写点分 `<分类前缀>.<英文短名>`，厂商硬件模板为 `hw.<族>.<编码>`，
//! 缩放模板为 `scale.<宽>x<高>`。
//! 文档 §11.1 为 Rust 版**建议**过一组 `sys.*` key（如 `sys.mp4-h264-auto`），与下表一一对应，
//! 仅前缀命名不同（本文档按分类前缀命名，便于按分类批量处理）。
//!
//! ## key → 名称（51 条，登记顺序 = `sort_order` 顺序）
//!
//! | key | 名称 | 分类 |
//! |---|---|---|
//! | `rec.mp4_h264_auto` | MP4 · H.264（自动选择最佳编码器） | 推荐 |
//! | `rec.mp4_hevc_auto` | MP4 · H.265 / HEVC（自动选择最佳编码器） | 推荐 |
//! | `rec.audio_mp3` | 音频 · 提取为 MP3 | 推荐 |
//! | `rec.fast_remux` | 快速换容器（不改画质，秒完成） | 推荐 |
//! | `video.mp4_x264` | 转换为 MP4（H.264 软件编码 x264) | 视频转码 |
//! | `video.mkv_x265` | 转换为 MKV（H.265 软件编码 x265) | 视频转码 |
//! | `video.webm_vp9` | 转换为 WebM（VP9 + Opus） | 视频转码 |
//! | `video.av1_svt` | 转换为 AV1（SVT-AV1，体积最小) | 视频转码 |
//! | `video.mov_prores` | 转换为 MOV（ProRes 422，剪辑友好） | 视频转码 |
//! | `video.gif` | 转换为 GIF 动图 | 视频转码 |
//! | `hw.h264_auto` | 硬件加速 · H.264（自动挑选可用编码器） | 硬件加速 |
//! | `hw.hevc_auto` | 硬件加速 · H.265（自动挑选可用编码器） | 硬件加速 |
//! | `hw.nvenc.h264` | NVIDIA NVENC · H.264 | 硬件加速 |
//! | `hw.nvenc.hevc` | NVIDIA NVENC · H.265 / HEVC | 硬件加速 |
//! | `hw.qsv.h264` | Intel Quick Sync · H.264 | 硬件加速 |
//! | `hw.qsv.hevc` | Intel Quick Sync · H.265 / HEVC | 硬件加速 |
//! | `hw.amf.h264` | AMD AMF · H.264 | 硬件加速 |
//! | `hw.amf.hevc` | AMD AMF · H.265 / HEVC | 硬件加速 |
//! | `hw.videotoolbox.h264` | Apple VideoToolbox · H.264 | 硬件加速 |
//! | `hw.videotoolbox.hevc` | Apple VideoToolbox · H.265 / HEVC | 硬件加速 |
//! | `hw.vaapi.h264` | Linux VA-API · H.264 | 硬件加速 |
//! | `hw.vaapi.hevc` | Linux VA-API · H.265 / HEVC | 硬件加速 |
//! | `compress.crf18` | 压缩 · 高画质（CRF 18) | 视频压缩 |
//! | `compress.crf23` | 压缩 · 均衡（CRF 23) | 视频压缩 |
//! | `compress.crf28` | 压缩 · 小体积（CRF 28) | 视频压缩 |
//! | `compress.max_size` | 压缩 · 限制文件大小 | 视频压缩 |
//! | `scale.3840x2160` | 调整为 4K（3840×2160） | 分辨率与画面 |
//! | `scale.1920x1080` | 调整为 1080p（1920×1080） | 分辨率与画面 |
//! | `scale.1280x720` | 调整为 720p（1280×720） | 分辨率与画面 |
//! | `scale.854x480` | 调整为 480p（854×480） | 分辨率与画面 |
//! | `scale.width` | 缩放 · 指定宽度（高度自适应） | 分辨率与画面 |
//! | `crop.vertical_9x16` | 裁剪 · 竖屏 9:16（1080×1920） | 分辨率与画面 |
//! | `rotate.90` | 旋转 · 顺时针 90° | 分辨率与画面 |
//! | `speed.change` | 变速 · 修改播放速度 | 分辨率与画面 |
//! | `deinterlace.yadif` | 去隔行 · 消除交错的横纹 | 分辨率与画面 |
//! | `audio.aac_m4a` | 音频 · 提取为 AAC / M4A | 音频处理 |
//! | `audio.flac` | 音频 · 提取为 FLAC（无损） | 音频处理 |
//! | `audio.wav` | 音频 · 提取为 WAV（PCM 无损） | 音频处理 |
//! | `audio.opus` | 音频 · 提取为 Opus | 音频处理 |
//! | `audio.loudnorm` | 音频 · 音量标准化（EBU R128） | 音频处理 |
//! | `audio.remove_track` | 视频 · 移除音轨（静音视频） | 音频处理 |
//! | `special.watermark_image` | 画面 · 添加图片水印 | 特殊处理 |
//! | `special.burn_subtitles` | 画面 · 烧录字幕（硬字幕） | 特殊处理 |
//! | `special.trim_segment` | 画面 · 按时间裁剪片段 | 特殊处理 |
//! | `special.image_sequence` | 画面 · 导出为图片序列（JPG） | 特殊处理 |
//! | `special.thumbnail` | 画面 · 生成封面缩略图 | 特殊处理 |
//! | `special.faststart` | 网络优化 · 仅重建索引（faststart） | 特殊处理 |
//! | `special.fix_timestamps` | 网络优化 · 无损调整时间戳（修正音画不同步） | 特殊处理 |
//! | `special.soft_subtitle_track` | 字幕 · 封装为可选字幕轨（不重新编码） | 特殊处理 |
//! | `special.keep_all_tracks` | 多音轨 · 保留全部音轨与字幕 | 特殊处理 |
//! | `audio.copy_track` | 音频 · 直接复制音轨（不重新编码） | 音频处理 |

use std::collections::HashMap;
use std::sync::OnceLock;

use sha2::{Digest, Sha256};

use crate::models::enums::{HardwareAccelerationMode, TemplateParameterKind};
use crate::models::{CommandTemplate, TemplateParameter, TemplateParameterOption, TemplateType};

/// 系统预设的版本号；升级应用时据此增量同步内置模板
///
/// 参考实现 `SystemTemplatePresets.Version`（`SystemTemplatePresets.cs:13`）。
/// **修改任何预设内容都必须递增它**，否则已有安装不会同步。
pub const PRESET_VERSION: i32 = 4;

/// 分类的展示顺序（不在列表里的分类排在最后，按名称排序）
pub const CATEGORY_ORDER: &[&str] = &[
    "推荐",
    "视频转码",
    "硬件加速",
    "视频压缩",
    "分辨率与画面",
    "音频处理",
    "特殊处理",
];

/// 全部内置模板的稳定 key（按登记顺序，即 `sort_order` 升序）。
///
/// id 由 [`system_template_id`] 从 key 派生（`SYSTEM_KEYS: &[(key, id)]` 的 key 侧，
/// id 侧由确定性哈希给出，无需硬编码）。测试会断言它与 [`all`] 的目录一一对应。
pub const SYSTEM_KEYS: &[&str] = &[
    "rec.mp4_h264_auto",
    "rec.mp4_hevc_auto",
    "rec.audio_mp3",
    "rec.fast_remux",
    "video.mp4_x264",
    "video.mkv_x265",
    "video.webm_vp9",
    "video.av1_svt",
    "video.mov_prores",
    "video.gif",
    "hw.h264_auto",
    "hw.hevc_auto",
    "hw.nvenc.h264",
    "hw.nvenc.hevc",
    "hw.qsv.h264",
    "hw.qsv.hevc",
    "hw.amf.h264",
    "hw.amf.hevc",
    "hw.videotoolbox.h264",
    "hw.videotoolbox.hevc",
    "hw.vaapi.h264",
    "hw.vaapi.hevc",
    "compress.crf18",
    "compress.crf23",
    "compress.crf28",
    "compress.max_size",
    "scale.3840x2160",
    "scale.1920x1080",
    "scale.1280x720",
    "scale.854x480",
    "scale.width",
    "crop.vertical_9x16",
    "rotate.90",
    "speed.change",
    "deinterlace.yadif",
    "audio.aac_m4a",
    "audio.flac",
    "audio.wav",
    "audio.opus",
    "audio.loudnorm",
    "audio.remove_track",
    "special.watermark_image",
    "special.burn_subtitles",
    "special.trim_segment",
    "special.image_sequence",
    "special.thumbnail",
    "special.faststart",
    "special.fix_timestamps",
    "special.soft_subtitle_track",
    "special.keep_all_tracks",
    "audio.copy_track",
];

/// 稳定 key → 确定性的 24 位十六进制 id。
///
/// 算法：`SHA-256(key)` 取**前 12 字节**，小写十六进制输出（24 个字符，与 `ObjectId` 字符串格式一致）。
///
/// ⚠️ 该映射一旦发布就**不可更改**：库里已有的系统模板靠它定位。
/// 新增模板只能追加新 key，不能改动已有 key（改 key 等于换 id，会留下陈旧条目）。
pub fn system_template_id(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let mut out = String::with_capacity(24);
    for byte in &digest.as_slice()[..12] {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 一个模板的稳定 key（库中模板 → 预设的定位依据）。
///
/// 先按 `id` 反查（本移植写入的记录）；查不到时**按 `name` 回退**，
/// 用于兼容参考实现（C#）写入的数据文件 —— 那里每条 `Id` 都是随机 `ObjectId`。
/// 参考实现里 `name` 全局唯一（§13.3），因此按名回退是确定的。
pub fn stable_key(template: &CommandTemplate) -> Option<String> {
    if let Some(key) = id_to_key().get(template.id.as_str()) {
        return Some((*key).to_string());
    }
    name_to_key()
        .get(template.name.as_str())
        .map(|key| (*key).to_string())
}

/// [`stable_key`] 的同义别名（同步算法用 key 定位同一条预设）。
pub fn key_of(template: &CommandTemplate) -> Option<String> {
    stable_key(template)
}

/// `(稳定 key, 模板)` 列表，顺序与 [`all`] 一致（供同步算法使用）。
pub fn all_with_keys() -> Vec<(String, CommandTemplate)> {
    all()
        .into_iter()
        .map(|t| {
            let key = stable_key(&t).unwrap_or_default();
            (key, t)
        })
        .collect()
}

/// 全部内置模板（按 `sort_order` 升序；`sort_order` 相同时按名称升序）
pub fn all() -> Vec<CommandTemplate> {
    let mut catalog = Catalog::new();
    build_catalog(&mut catalog);
    let mut templates = catalog.finish();
    templates.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then_with(|| a.name.cmp(&b.name)));
    templates
}

fn id_to_key() -> &'static HashMap<String, &'static str> {
    static MAP: OnceLock<HashMap<String, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| {
        SYSTEM_KEYS
            .iter()
            .map(|key| (system_template_id(key), *key))
            .collect()
    })
}

fn name_to_key() -> &'static HashMap<String, &'static str> {
    static MAP: OnceLock<HashMap<String, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| {
        all()
            .into_iter()
            .filter_map(|t| {
                id_to_key()
                    .get(t.id.as_str())
                    .map(|key| (t.name, *key))
            })
            .collect()
    })
}

// ── 目录构建（对应 C# `CreateAll()` 里的局部函数 `Add` 与两个生成器） ───────────────

/// [`Catalog::add`] 的可选参数（对应 C# 局部函数 `Add` 的具名可选参数）。
///
/// 位置参数顺序与参考实现一致：`category, name, description, args, extension`，
/// 其余走本结构体：`inputFormats, targetCodec, hardwareMode, requiredEncoder, parameters,
/// tags, recommended, requiredFilters`。
#[derive(Default)]
struct AddOptions<'a> {
    /// `SupportedInputFormats`（`[]` = 不限）
    input_formats: &'a [&'a str],
    /// `TargetCodec`（`""` = 未指定）
    target_codec: &'a str,
    /// `HardwareMode`
    hardware_mode: HardwareAccelerationMode,
    /// `RequiredEncoder`（族名或具体编码器名）
    required_encoder: Option<&'a str>,
    /// `Parameters`
    parameters: Vec<TemplateParameter>,
    /// `Tags`
    tags: Vec<&'a str>,
    /// `IsRecommended`
    recommended: bool,
    /// `RequiredFilters`
    required_filters: &'a [&'a str],
}

/// 预设目录累加器：负责 `sort_order` 自增与稳定 id 注入。
struct Catalog {
    templates: Vec<CommandTemplate>,
}

impl Catalog {
    fn new() -> Self {
        Self { templates: Vec::new() }
    }

    /// 等价于 C# 的局部函数 `Add`（`SystemTemplatePresets.cs:33-70`）+ `templates.Add`。
    ///
    /// 与参考实现唯一的差别：`Id` 用 `system_template_id(key)` 取代随机 `ObjectId`，
    /// 其余字段（含 `SortOrder = order++`、`CreatedAt/UpdatedAt = UtcNow`）逐字对应。
    fn add(
        &mut self,
        key: &str,
        category: &str,
        name: &str,
        description: &str,
        args: &str,
        extension: &str,
        opts: AddOptions<'_>,
    ) {
        let now = chrono::Utc::now();
        let order = self.templates.len() as i32; // C# 的 `order++`
        self.templates.push(CommandTemplate {
            id: system_template_id(key),
            name: name.to_string(),
            description: description.to_string(),
            command_args: args.to_string(),
            r#type: TemplateType::System,
            category: category.to_string(),
            supported_input_formats: opts.input_formats.iter().map(|s| s.to_string()).collect(),
            output_extension: extension.to_string(),
            target_codec: opts.target_codec.to_string(),
            // C#: RequiresHardwareAcceleration = hardwareMode == Required
            requires_hardware_acceleration: opts.hardware_mode == HardwareAccelerationMode::Required,
            hardware_mode: opts.hardware_mode,
            required_encoder: opts.required_encoder.map(|s| s.to_string()),
            parameters: opts.parameters,
            required_filters: opts.required_filters.iter().map(|s| s.to_string()).collect(),
            tags: opts.tags.iter().map(|s| s.to_string()).collect(),
            is_recommended: opts.recommended,
            preset_version: PRESET_VERSION,
            created_at: now,
            updated_at: now,
            sort_order: order,
        });
    }

    fn finish(self) -> Vec<CommandTemplate> {
        self.templates
    }
}

/// 厂商硬件族（对应 C# 5 次 `AddHardwareFamily` 调用，顺序固定，不可调整）。
///
/// `(family = required_encoder, displayFamily, vendorDescription)`
const HARDWARE_FAMILIES: &[(&str, &str, &str)] = &[
    ("nvenc", "NVIDIA NVENC", "NVIDIA 显卡"),
    ("qsv", "Intel Quick Sync", "Intel 核显 / 独显"),
    ("amf", "AMD AMF", "AMD 显卡"),
    ("videotoolbox", "Apple VideoToolbox", "Apple Silicon / macOS"),
    ("vaapi", "Linux VA-API", "Linux 下的 Intel / AMD 显卡"),
];

/// 缩放模板（对应 C# 4 次 `AddScaleTemplate` 调用，顺序固定）。
///
/// `(key, name, width, height)`
const SCALE_TEMPLATES: &[(&str, &str, i32, i32)] = &[
    ("scale.3840x2160", "调整为 4K（3840×2160）", 3840, 2160),
    ("scale.1920x1080", "调整为 1080p（1920×1080）", 1920, 1080),
    ("scale.1280x720", "调整为 720p（1280×720）", 1280, 720),
    ("scale.854x480", "调整为 480p（854×480）", 854, 480),
];

/// 生成某一硬件族的 H.264 / H.265 模板（等价于 `AddHardwareFamily`，`SystemTemplatePresets.cs:599-643`）。
///
/// 注意：`hardQuality` / `audio` 两个参数对象在参考实现里是**每次调用新建**，
/// 这里每次调用重新构造，语义一致。
fn add_hardware_family(c: &mut Catalog, display_family: &str, family: &str, vendor_description: &str) {
    let hard_quality = || TemplateParameter {
        name: "quality".to_string(),
        label: "质量".to_string(),
        description: "留空使用该编码器的推荐默认值。".to_string(),
        kind: TemplateParameterKind::Select,
        default_value: String::new(),
        options: vec![
            opt("", "自动（推荐）"),
            opt("20", "高画质"),
            opt("24", "均衡"),
            opt("28", "小体积"),
            opt("32", "极小体积"),
        ],
        ..Default::default()
    };
    let audio = || TemplateParameter {
        name: "audio_bitrate".to_string(),
        label: "音频码率".to_string(),
        kind: TemplateParameterKind::Select,
        default_value: "128k".to_string(),
        options: vec![
            opt("96k", "96 kbps"),
            opt("128k", "128 kbps"),
            opt("192k", "192 kbps"),
            opt("256k", "256 kbps"),
        ],
        ..Default::default()
    };

    c.add(
        &format!("hw.{family}.h264"),
        "硬件加速",
        &format!("{display_family} · H.264"),
        &format!("使用 {vendor_description} 的 {display_family} 硬件编码 H.264，速度快、CPU 占用低。"),
        "-i \"{input}\" -c:v {encoder} {quality_args} {preset_args} {hwupload_args} \
         -c:a {audio_encoder} -b:a {audio_bitrate} -movflags +faststart \"{output}\"",
        "mp4",
        AddOptions {
            target_codec: "h264",
            hardware_mode: HardwareAccelerationMode::Required,
            required_encoder: Some(family),
            parameters: vec![hard_quality(), audio()],
            tags: vec!["硬件加速", "h264", family],
            ..Default::default()
        },
    );

    c.add(
        &format!("hw.{family}.hevc"),
        "硬件加速",
        &format!("{display_family} · H.265 / HEVC"),
        &format!("使用 {vendor_description} 的 {display_family} 硬件编码 H.265，画质/体积比更好。"),
        "-i \"{input}\" -c:v {encoder} {quality_args} {preset_args} {hwupload_args} \
         -c:a {audio_encoder} -b:a {audio_bitrate} -tag:v hvc1 -movflags +faststart \"{output}\"",
        "mp4",
        AddOptions {
            target_codec: "hevc",
            hardware_mode: HardwareAccelerationMode::Required,
            required_encoder: Some(family),
            parameters: vec![hard_quality(), audio()],
            tags: vec!["硬件加速", "hevc", family],
            ..Default::default()
        },
    );
}

/// 生成一个分辨率缩放模板（等价于 `AddScaleTemplate`，`SystemTemplatePresets.cs:645-657`）。
fn add_scale_template(c: &mut Catalog, key: &str, name: &str, width: i32, height: i32) {
    c.add(
        key,
        "分辨率与画面",
        name,
        &format!("等比缩放到不超过 {width}×{height}，必要时用黑边补齐，保证画面不变形。"),
        &format!(
            "-i \"{{input}}\" -vf \"scale={width}:{height}:force_original_aspect_ratio=decrease,\
             pad={width}:{height}:(ow-iw)/2:(oh-ih)/2,setsar=1\" \
             -c:v libx264 -crf 23 -preset medium -c:a aac -b:a 128k -movflags +faststart \"{{output}}\""
        ),
        "mp4",
        AddOptions {
            target_codec: "h264",
            tags: vec!["缩放", &format!("{height}p")],
            ..Default::default()
        },
    );
}

// ── 共享参数对象（对应 C# 的 `qualityAuto` / `audioBitrate`，逐字） ─────────────────

/// `TemplateParameterOption` 的 `(value, label)` 两参重载（`Description` 恒为 `""`）
fn opt(value: &str, label: &str) -> TemplateParameterOption {
    TemplateParameterOption::new(value, label, "")
}

/// `qualityAuto`（`SystemTemplatePresets.cs:72-87`，逐字）
fn quality_auto() -> TemplateParameter {
    TemplateParameter {
        name: "quality".to_string(),
        label: "质量".to_string(),
        description: "数值越小画质越好、文件越大。留空则由当前编码器的最佳默认值决定。".to_string(),
        kind: TemplateParameterKind::Select,
        default_value: String::new(),
        options: vec![
            opt("", "自动（推荐）"),
            opt("18", "高画质"),
            opt("23", "均衡"),
            opt("28", "小体积"),
            opt("32", "极小体积"),
        ],
        ..Default::default()
    }
}

/// `audioBitrate`（`SystemTemplatePresets.cs:89-99`，逐字）
fn audio_bitrate() -> TemplateParameter {
    TemplateParameter {
        name: "audio_bitrate".to_string(),
        label: "音频码率".to_string(),
        kind: TemplateParameterKind::Select,
        default_value: "128k".to_string(),
        options: vec![
            opt("96k", "96 kbps"),
            opt("128k", "128 kbps"),
            opt("192k", "192 kbps"),
            opt("256k", "256 kbps"),
        ],
        ..Default::default()
    }
}

fn select_param(
    name: &str,
    label: &str,
    description: &str,
    default_value: &str,
    options: Vec<TemplateParameterOption>,
) -> TemplateParameter {
    TemplateParameter {
        name: name.to_string(),
        label: label.to_string(),
        description: description.to_string(),
        kind: TemplateParameterKind::Select,
        default_value: default_value.to_string(),
        options,
        ..Default::default()
    }
}

fn text_param(name: &str, label: &str, description: &str, default_value: &str) -> TemplateParameter {
    TemplateParameter {
        name: name.to_string(),
        label: label.to_string(),
        description: description.to_string(),
        kind: TemplateParameterKind::Text,
        default_value: default_value.to_string(),
        ..Default::default()
    }
}

fn number_param(
    name: &str,
    label: &str,
    description: &str,
    default_value: &str,
    min: f64,
    max: f64,
) -> TemplateParameter {
    TemplateParameter {
        name: name.to_string(),
        label: label.to_string(),
        description: description.to_string(),
        kind: TemplateParameterKind::Number,
        default_value: default_value.to_string(),
        min: Some(min),
        max: Some(max),
        ..Default::default()
    }
}

/// 建立全部 51 个预设（严格按参考实现的登记顺序，顺序即 `sort_order`）。
fn build_catalog(c: &mut Catalog) {
    // ── 推荐 ───────────────────────────────────────────────────
    c.add(
        "rec.mp4_h264_auto",
        "推荐",
        "MP4 · H.264（自动选择最佳编码器）",
        "输出兼容性最好的 H.264 MP4。会自动使用本机可用的硬件编码器（NVENC / Quick Sync / AMF / \
         VideoToolbox / VA-API），没有硬件时回退到 x264。",
        "-i \"{input}\" -c:v {encoder} {quality_args} {preset_args} {hwupload_args} \
         -map 0:v:0 -map 0:a? -c:a {audio_encoder} -b:a {audio_bitrate} \
         -movflags +faststart -pix_fmt yuv420p \"{output}\"",
        "mp4",
        AddOptions {
            target_codec: "h264",
            hardware_mode: HardwareAccelerationMode::Auto,
            parameters: vec![quality_auto(), audio_bitrate()],
            tags: vec!["mp4", "h264", "通用", "硬件加速"],
            recommended: true,
            ..Default::default()
        },
    );

    c.add(
        "rec.mp4_hevc_auto",
        "推荐",
        "MP4 · H.265 / HEVC（自动选择最佳编码器）",
        "同等画质下体积比 H.264 小约 40%。会自动使用本机可用的硬件编码器，没有硬件时回退到 x265。",
        "-i \"{input}\" -c:v {encoder} {quality_args} {preset_args} {hwupload_args} \
         -map 0:v:0 -map 0:a? -c:a {audio_encoder} -b:a {audio_bitrate} \
         -tag:v hvc1 -movflags +faststart \"{output}\"",
        "mp4",
        AddOptions {
            target_codec: "hevc",
            hardware_mode: HardwareAccelerationMode::Auto,
            parameters: vec![quality_auto(), audio_bitrate()],
            tags: vec!["mp4", "hevc", "压缩", "硬件加速"],
            recommended: true,
            ..Default::default()
        },
    );

    c.add(
        "rec.audio_mp3",
        "推荐",
        "音频 · 提取为 MP3",
        "从视频或音频中提取音轨并转成通用性最好的 MP3。",
        "-i \"{input}\" -vn -map 0:a:0 -c:a libmp3lame -b:a {audio_bitrate} \"{output}\"",
        "mp3",
        AddOptions {
            input_formats: &[
                "mp4", "mkv", "avi", "mov", "webm", "flv", "ts", "wav", "flac", "m4a", "aac", "ogg",
                "wmv",
            ],
            parameters: vec![audio_bitrate()],
            tags: vec!["mp3", "音频", "提取"],
            recommended: true,
            ..Default::default()
        },
    );

    c.add(
        "rec.fast_remux",
        "推荐",
        "快速换容器（不改画质，秒完成）",
        "只重新封装容器，不重新编码，速度极快、画质完全无损。仅当目标容器支持源编码格式时可用。",
        "-i \"{input}\" -map 0 -c copy -movflags +faststart \"{output}\"",
        "mp4",
        AddOptions {
            tags: vec!["封装", "无损", "快速"],
            recommended: true,
            ..Default::default()
        },
    );

    // ── 视频转码 ────────────────────────────────────────────────
    c.add(
        "video.mp4_x264",
        "视频转码",
        "转换为 MP4（H.264 软件编码 x264)",
        "使用 CPU 软件编码，画质与体积控制最精细，兼容所有平台。",
        "-i \"{input}\" -c:v libx264 -crf {quality} -preset {preset} -c:a aac -b:a {audio_bitrate} \
         -movflags +faststart -pix_fmt yuv420p \"{output}\"",
        "mp4",
        AddOptions {
            input_formats: &["avi", "mkv", "mov", "wmv", "flv", "webm", "ts", "mpg", "m4v", "3gp"],
            parameters: vec![
                select_param(
                    "quality",
                    "CRF 质量",
                    "18 接近无损，23 为默认均衡值，28 体积明显更小。",
                    "23",
                    vec![opt("18", "18 · 高画质"), opt("23", "23 · 均衡"), opt("28", "28 · 小体积")],
                ),
                select_param(
                    "preset",
                    "编码速度",
                    "越慢压缩率越高（体积更小）。",
                    "medium",
                    vec![
                        opt("veryfast", "很快"),
                        opt("fast", "快"),
                        opt("medium", "中等（推荐）"),
                        opt("slow", "慢 · 更小"),
                        opt("veryslow", "很慢 · 最小"),
                    ],
                ),
                audio_bitrate(),
            ],
            tags: vec!["mp4", "h264", "软件编码"],
            ..Default::default()
        },
    );

    c.add(
        "video.mkv_x265",
        "视频转码",
        "转换为 MKV（H.265 软件编码 x265)",
        "MKV 容器 + x265 软件编码，适合本地归档保存。",
        "-i \"{input}\" -c:v libx265 -crf {quality} -preset {preset} -c:a aac -b:a {audio_bitrate} \
         \"{output}\"",
        "mkv",
        AddOptions {
            parameters: vec![
                select_param(
                    "quality",
                    "CRF 质量",
                    "x265 的 CRF 通常比 x264 高 3~5。",
                    "28",
                    vec![opt("22", "22 · 高画质"), opt("28", "28 · 均衡"), opt("33", "33 · 小体积")],
                ),
                select_param(
                    "preset",
                    "编码速度",
                    "",
                    "medium",
                    vec![
                        opt("veryfast", "很快"),
                        opt("medium", "中等（推荐）"),
                        opt("slow", "慢 · 更小"),
                    ],
                ),
                audio_bitrate(),
            ],
            tags: vec!["mkv", "hevc", "归档"],
            ..Default::default()
        },
    );

    c.add(
        "video.webm_vp9",
        "视频转码",
        "转换为 WebM（VP9 + Opus）",
        "开源免专利格式，适合网页嵌入。",
        "-i \"{input}\" -c:v libvpx-vp9 -crf {quality} -b:v 0 -row-mt 1 -c:a libopus \
         -b:a {audio_bitrate} \"{output}\"",
        "webm",
        AddOptions {
            parameters: vec![
                select_param(
                    "quality",
                    "CRF 质量",
                    "",
                    "32",
                    vec![opt("24", "24 · 高画质"), opt("32", "32 · 均衡"), opt("40", "40 · 小体积")],
                ),
                audio_bitrate(),
            ],
            tags: vec!["webm", "vp9"],
            ..Default::default()
        },
    );

    c.add(
        "video.av1_svt",
        "视频转码",
        "转换为 AV1（SVT-AV1，体积最小)",
        "压缩率最高的开放格式，编码速度较慢但体积最小。需要 FFmpeg 编译了 libsvtav1。",
        "-i \"{input}\" -c:v libsvtav1 -crf {quality} -preset {preset} -c:a aac -b:a {audio_bitrate} \
         -movflags +faststart \"{output}\"",
        "mp4",
        AddOptions {
            target_codec: "av1",
            parameters: vec![
                select_param(
                    "quality",
                    "CRF 质量",
                    "",
                    "32",
                    vec![opt("24", "24 · 高画质"), opt("32", "32 · 均衡"), opt("40", "40 · 小体积")],
                ),
                select_param(
                    "preset",
                    "编码速度",
                    "数字越大越快、压缩率越低。",
                    "8",
                    vec![
                        opt("4", "4 · 最慢/最小"),
                        opt("8", "8 · 均衡（推荐）"),
                        opt("12", "12 · 快"),
                    ],
                ),
                audio_bitrate(),
            ],
            tags: vec!["av1", "mp4", "压缩"],
            ..Default::default()
        },
    );

    c.add(
        "video.mov_prores",
        "视频转码",
        "转换为 MOV（ProRes 422，剪辑友好）",
        "生成适合 Final Cut / Premiere 剪辑的中间码率素材，体积较大。",
        "-i \"{input}\" -c:v prores_ks -profile:v 3 -vendor apl0 -pix_fmt yuv422p10le \
         -c:a pcm_s16le \"{output}\"",
        "mov",
        AddOptions {
            target_codec: "prores",
            tags: vec!["mov", "prores", "剪辑"],
            ..Default::default()
        },
    );

    c.add(
        "video.gif",
        "视频转码",
        "转换为 GIF 动图",
        "生成高质量调色板的 GIF，默认限制在 480 宽、12fps。",
        "-i \"{input}\" -vf \"fps={fps},scale={width}:-2:flags=lanczos,split[s0][s1];\
         [s0]palettegen[p];[s1][p]paletteuse\" -loop 0 \"{output}\"",
        "gif",
        AddOptions {
            parameters: vec![
                select_param(
                    "fps",
                    "帧率",
                    "",
                    "12",
                    vec![
                        opt("8", "8 fps · 最小"),
                        opt("12", "12 fps · 推荐"),
                        opt("15", "15 fps"),
                        opt("25", "25 fps · 最流畅"),
                    ],
                ),
                select_param(
                    "width",
                    "宽度",
                    "高度按比例自动计算且保证为偶数。",
                    "480",
                    vec![opt("320", "320 px"), opt("480", "480 px"), opt("640", "640 px"), opt("800", "800 px")],
                ),
            ],
            tags: vec!["gif", "动图"],
            ..Default::default()
        },
    );

    // ── 硬件加速 ────────────────────────────────────────────────
    c.add(
        "hw.h264_auto",
        "硬件加速",
        "硬件加速 · H.264（自动挑选可用编码器）",
        "自动使用本机最合适的 H.264 硬件编码器；模板会带上对应编码器的正确质量参数。",
        "-i \"{input}\" -c:v {encoder} {quality_args} {preset_args} {hwupload_args} \
         -c:a {audio_encoder} -b:a {audio_bitrate} -movflags +faststart \"{output}\"",
        "mp4",
        AddOptions {
            target_codec: "h264",
            hardware_mode: HardwareAccelerationMode::Auto,
            parameters: vec![quality_auto(), audio_bitrate()],
            tags: vec!["硬件加速", "h264"],
            ..Default::default()
        },
    );

    c.add(
        "hw.hevc_auto",
        "硬件加速",
        "硬件加速 · H.265（自动挑选可用编码器）",
        "自动使用本机最合适的 H.265 硬件编码器，画质/体积比优于 H.264。",
        "-i \"{input}\" -c:v {encoder} {quality_args} {preset_args} {hwupload_args} \
         -c:a {audio_encoder} -b:a {audio_bitrate} -tag:v hvc1 -movflags +faststart \"{output}\"",
        "mp4",
        AddOptions {
            target_codec: "hevc",
            hardware_mode: HardwareAccelerationMode::Auto,
            parameters: vec![quality_auto(), audio_bitrate()],
            tags: vec!["硬件加速", "hevc"],
            ..Default::default()
        },
    );

    for (family, display_family, vendor_description) in HARDWARE_FAMILIES {
        add_hardware_family(c, display_family, family, vendor_description);
    }

    // ── 视频压缩 ────────────────────────────────────────────────
    c.add(
        "compress.crf18",
        "视频压缩",
        "压缩 · 高画质（CRF 18)",
        "肉眼几乎无损失，体积约为原文件的一半。",
        "-i \"{input}\" -c:v libx264 -crf 18 -preset slow -c:a aac -b:a 192k \
         -movflags +faststart \"{output}\"",
        "mp4",
        AddOptions {
            tags: vec!["压缩"],
            ..Default::default()
        },
    );

    c.add(
        "compress.crf23",
        "视频压缩",
        "压缩 · 均衡（CRF 23)",
        "在画质和体积之间取得平衡，适合大多数场景。",
        "-i \"{input}\" -c:v libx264 -crf 23 -preset medium -c:a aac -b:a 128k \
         -movflags +faststart \"{output}\"",
        "mp4",
        AddOptions {
            tags: vec!["压缩"],
            ..Default::default()
        },
    );

    c.add(
        "compress.crf28",
        "视频压缩",
        "压缩 · 小体积（CRF 28)",
        "明显缩小体积，适合手机观看或分享。",
        "-i \"{input}\" -c:v libx264 -crf 28 -preset fast -c:a aac -b:a 96k \
         -movflags +faststart \"{output}\"",
        "mp4",
        AddOptions {
            tags: vec!["压缩"],
            ..Default::default()
        },
    );

    c.add(
        "compress.max_size",
        "视频压缩",
        "压缩 · 限制文件大小",
        "按目标体积限制输出文件大小，适合有上传大小限制的场景。",
        "-i \"{input}\" -c:v libx264 -preset fast -crf 26 -maxrate {maxrate} -bufsize {bufsize} \
         -fs {maxsize}M -c:a aac -b:a 96k -movflags +faststart \"{output}\"",
        "mp4",
        AddOptions {
            parameters: vec![
                number_param("maxsize", "目标大小 (MB)", "达到该大小后 FFmpeg 会停止写入。", "50", 1.0, 100_000.0),
                select_param(
                    "maxrate",
                    "最大码率",
                    "",
                    "1500k",
                    vec![
                        opt("800k", "800 kbps"),
                        opt("1500k", "1500 kbps"),
                        opt("3000k", "3000 kbps"),
                        opt("6000k", "6000 kbps"),
                    ],
                ),
                select_param(
                    "bufsize",
                    "缓冲区",
                    "",
                    "3000k",
                    vec![
                        opt("1600k", "1600 kbps"),
                        opt("3000k", "3000 kbps"),
                        opt("6000k", "6000 kbps"),
                        opt("12000k", "12000 kbps"),
                    ],
                ),
            ],
            tags: vec!["压缩", "体积限制"],
            ..Default::default()
        },
    );

    // ── 分辨率与画面 ────────────────────────────────────────────
    for (key, name, width, height) in SCALE_TEMPLATES {
        add_scale_template(c, key, name, *width, *height);
    }

    c.add(
        "scale.width",
        "分辨率与画面",
        "缩放 · 指定宽度（高度自适应）",
        "只指定宽度，高度按原始比例自动计算，并保证为偶数。",
        "-i \"{input}\" -vf \"scale={width}:-2:flags=lanczos\" -c:v libx264 -crf 23 -c:a aac \
         -b:a 128k -movflags +faststart \"{output}\"",
        "mp4",
        AddOptions {
            parameters: vec![select_param(
                "width",
                "目标宽度 (px)",
                "",
                "1280",
                vec![
                    opt("640", "640"),
                    opt("854", "854"),
                    opt("1280", "1280"),
                    opt("1920", "1920"),
                    opt("2560", "2560"),
                    opt("3840", "3840"),
                ],
            )],
            tags: vec!["缩放"],
            ..Default::default()
        },
    );

    c.add(
        "crop.vertical_9x16",
        "分辨率与画面",
        "裁剪 · 竖屏 9:16（1080×1920）",
        "居中裁剪为竖屏短视频比例，适合抖音 / 视频号 / Shorts。",
        "-i \"{input}\" -vf \"scale=1080:1920:force_original_aspect_ratio=increase,crop=1080:1920\" \
         -c:v libx264 -crf 23 -c:a aac -b:a 128k -movflags +faststart \"{output}\"",
        "mp4",
        AddOptions {
            tags: vec!["竖屏", "裁剪"],
            ..Default::default()
        },
    );

    c.add(
        "rotate.90",
        "分辨率与画面",
        "旋转 · 顺时针 90°",
        "把横屏视频旋转为竖屏（或反过来）。",
        // ⚠️ 逐字保留参考实现的缺陷：命令里写死 `transpose=1`，参数 `angle` 没有任何占位符引用它
        // （死参数，选「逆时针 90°」不会改变命令）。§13.2 建议改成 `transpose={angle}`，但本次移植
        // 要求逐字一致，故此处**故意不改**。唯一与 `angle` 相关的测试例外见 `parameter_placeholders_exist`。
        "-i \"{input}\" -vf \"transpose=1\" -c:v libx264 -crf 23 -c:a aac -b:a 128k \
         -movflags +faststart \"{output}\"",
        "mp4",
        AddOptions {
            parameters: vec![select_param(
                "angle",
                "方向",
                "对应 FFmpeg transpose 参数。",
                "1",
                vec![opt("1", "顺时针 90°"), opt("2", "逆时针 90°")],
            )],
            tags: vec!["旋转"],
            ..Default::default()
        },
    );

    c.add(
        "speed.change",
        "分辨率与画面",
        "变速 · 修改播放速度",
        "通过调整时间戳实现无音调变化的变速（视频不变调，音频会被重采样）。",
        "-i \"{input}\" -filter_complex \"[0:v]setpts=PTS/{speed}[v];[0:a]atempo={atempo}[a]\" \
         -map \"[v]\" -map \"[a]\" -c:v libx264 -crf 23 -c:a aac -b:a 128k \"{output}\"",
        "mp4",
        AddOptions {
            parameters: vec![
                select_param(
                    "speed",
                    "速度倍率",
                    "大于 1 变快，小于 1 变慢。",
                    "2",
                    vec![
                        opt("0.5", "0.5× 慢放"),
                        opt("1.5", "1.5×"),
                        opt("2", "2× 快放"),
                        opt("4", "4×"),
                    ],
                ),
                select_param(
                    "atempo",
                    "音频同步倍率",
                    "需要与速度倍率一致，音频允许的范围是 0.5~100。",
                    "2",
                    vec![opt("0.5", "0.5×"), opt("1.5", "1.5×"), opt("2", "2×"), opt("4", "4×")],
                ),
            ],
            tags: vec!["变速"],
            ..Default::default()
        },
    );

    c.add(
        "deinterlace.yadif",
        "分辨率与画面",
        "去隔行 · 消除交错的横纹",
        "对老式摄像机/电视录制的隔行视频做反交错处理。",
        "-i \"{input}\" -vf \"yadif=1:-1:0\" -c:v libx264 -crf 20 -preset medium -c:a aac \
         -b:a 128k -movflags +faststart \"{output}\"",
        "mp4",
        AddOptions {
            tags: vec!["去隔行", "老视频"],
            ..Default::default()
        },
    );

    // ── 音频处理 ────────────────────────────────────────────────
    c.add(
        "audio.aac_m4a",
        "音频处理",
        "音频 · 提取为 AAC / M4A",
        "提取音轨并转为 AAC，体积小、兼容性好。",
        "-i \"{input}\" -vn -map 0:a:0 -c:a aac -b:a {audio_bitrate} \"{output}\"",
        "m4a",
        AddOptions {
            input_formats: &[
                "mp4", "mkv", "avi", "mov", "webm", "flv", "ts", "wav", "flac", "m4a", "aac", "ogg",
                "wmv",
            ],
            parameters: vec![audio_bitrate()],
            tags: vec!["音频", "aac"],
            ..Default::default()
        },
    );

    c.add(
        "audio.flac",
        "音频处理",
        "音频 · 提取为 FLAC（无损）",
        "无损提取音轨，不损失任何音质。",
        "-i \"{input}\" -vn -map 0:a:0 -c:a flac -compression_level 8 \"{output}\"",
        "flac",
        AddOptions {
            input_formats: &[
                "mp4", "mkv", "avi", "mov", "webm", "flv", "ts", "wav", "m4a", "aac", "ogg", "wmv",
            ],
            tags: vec!["音频", "无损"],
            ..Default::default()
        },
    );

    c.add(
        "audio.wav",
        "音频处理",
        "音频 · 提取为 WAV（PCM 无损）",
        "未压缩 PCM，适合后续在音频软件里继续处理。",
        "-i \"{input}\" -vn -map 0:a:0 -c:a pcm_s16le -ar 48000 \"{output}\"",
        "wav",
        AddOptions {
            input_formats: &[
                "mp4", "mkv", "avi", "mov", "webm", "flv", "ts", "flac", "m4a", "aac", "ogg", "wmv",
            ],
            tags: vec!["音频", "wav", "无损"],
            ..Default::default()
        },
    );

    c.add(
        "audio.opus",
        "音频处理",
        "音频 · 提取为 Opus",
        "同码率下音质优于 MP3/AAC，适合网络传输。",
        "-i \"{input}\" -vn -map 0:a:0 -c:a libopus -b:a 128k -vbr on \"{output}\"",
        "opus",
        AddOptions {
            input_formats: &[
                "mp4", "mkv", "avi", "mov", "webm", "flv", "ts", "wav", "flac", "m4a", "aac", "ogg",
            ],
            tags: vec!["音频", "opus"],
            ..Default::default()
        },
    );

    c.add(
        "audio.loudnorm",
        "音频处理",
        "音频 · 音量标准化（EBU R128）",
        "把整体响度标准化到 -16 LUFS，避免忽大忽小。",
        "-i \"{input}\" -vn -af \"loudnorm=I=-16:TP=-1.5:LRA=11\" -c:a aac -b:a 192k \"{output}\"",
        "m4a",
        AddOptions {
            input_formats: &[
                "mp4", "mkv", "avi", "mov", "webm", "flv", "ts", "wav", "flac", "m4a", "aac", "ogg",
                "mp3",
            ],
            tags: vec!["音频", "响度"],
            ..Default::default()
        },
    );

    c.add(
        "audio.remove_track",
        "音频处理",
        "视频 · 移除音轨（静音视频）",
        "彻底去掉音频流，只保留画面。",
        "-i \"{input}\" -an -c:v copy \"{output}\"",
        "mp4",
        AddOptions {
            tags: vec!["静音", "去音轨"],
            ..Default::default()
        },
    );

    // ── 特殊处理 ────────────────────────────────────────────────
    c.add(
        "special.watermark_image",
        "特殊处理",
        "画面 · 添加图片水印",
        "在右下角叠加一张 PNG/JPG 水印，可通过参数调整位置与透明度。",
        "-i \"{input}\" -i \"{watermark}\" -filter_complex \
         \"[1:v]format=rgba,colorchannelmixer=aa={opacity}[wm];[0:v][wm]overlay={position}\" \
         -c:v libx264 -crf 23 -preset medium -c:a copy \"{output}\"",
        "mp4",
        AddOptions {
            parameters: vec![
                text_param("watermark", "水印图片路径", "填写 PNG/JPG 的完整路径。", ""),
                select_param(
                    "position",
                    "水印位置",
                    "",
                    "W-w-20:H-h-20",
                    vec![
                        opt("W-w-20:H-h-20", "右下角"),
                        opt("20:20", "左上角"),
                        opt("W-w-20:20", "右上角"),
                        opt("20:H-h-20", "左下角"),
                        opt("(W-w)/2:(H-h)/2", "居中"),
                    ],
                ),
                select_param(
                    "opacity",
                    "不透明度",
                    "",
                    "1",
                    vec![opt("0.3", "30%"), opt("0.5", "50%"), opt("0.8", "80%"), opt("1", "不透明")],
                ),
            ],
            tags: vec!["水印", "叠加"],
            ..Default::default()
        },
    );

    c.add(
        "special.burn_subtitles",
        "特殊处理",
        "画面 · 烧录字幕（硬字幕）",
        // 描述在源码里是两段字面量拼接，中间没有空格
        "把字幕永久烧进画面，任何播放器都能显示。支持 srt/ass。\
         需要 FFmpeg 编译时启用 libass（Homebrew / winget / gyan.dev 的完整构建都有，部分精简构建没有）。",
        "-i \"{input}\" -vf \"subtitles=filename='{subtitle}':force_style='FontSize={fontsize}'\" \
         -c:v libx264 -crf 23 -preset medium -c:a copy \"{output}\"",
        "mp4",
        AddOptions {
            required_filters: &["subtitles"],
            parameters: vec![
                text_param("subtitle", "字幕文件路径", "srt / ass 文件的完整路径。路径中不要包含单引号。", ""),
                select_param(
                    "fontsize",
                    "字号",
                    "",
                    "24",
                    vec![opt("16", "小"), opt("24", "中"), opt("32", "大"), opt("40", "特大")],
                ),
            ],
            tags: vec!["字幕", "硬字幕"],
            ..Default::default()
        },
    );

    c.add(
        "special.trim_segment",
        "特殊处理",
        "画面 · 按时间裁剪片段",
        "截取视频中的一段，起止时间使用 时:分:秒 格式。",
        "-ss {start} -i \"{input}\" -t {duration} -c:v libx264 -crf 20 -preset medium -c:a aac \
         -b:a 128k -movflags +faststart \"{output}\"",
        "mp4",
        AddOptions {
            parameters: vec![
                text_param("start", "开始时间", "格式 时:分:秒，例如 00:01:30。", "00:00:00"),
                text_param("duration", "时长", "格式 时:分:秒，例如 00:00:30。", "00:00:30"),
            ],
            tags: vec!["裁剪", "剪辑"],
            ..Default::default()
        },
    );

    c.add(
        "special.image_sequence",
        "特殊处理",
        "画面 · 导出为图片序列（JPG）",
        "按指定帧率抽帧导出为一组 JPG。文件名会自动带上 4 位序号，\
         例如输出名填 result 会生成 result_0001.jpg、result_0002.jpg ……",
        "-i \"{input}\" -vf fps={fps} -q:v {jpgquality} \"{output_pattern}\"",
        "jpg",
        AddOptions {
            parameters: vec![
                select_param(
                    "fps",
                    "抽帧频率",
                    "",
                    "1",
                    vec![
                        opt("0.5", "每 2 秒 1 张"),
                        opt("1", "每秒 1 张"),
                        opt("2", "每秒 2 张"),
                        opt("5", "每秒 5 张"),
                    ],
                ),
                select_param(
                    "jpgquality",
                    "图片质量",
                    "数值越小画质越高（JPEG qscale）。",
                    "3",
                    vec![opt("2", "高"), opt("3", "中（推荐）"), opt("8", "小")],
                ),
            ],
            tags: vec!["抽帧", "图片"],
            ..Default::default()
        },
    );

    c.add(
        "special.thumbnail",
        "特殊处理",
        "画面 · 生成封面缩略图",
        "从指定时间点截取一张图片作为视频封面。",
        "-ss {seek} -i \"{input}\" -frames:v 1 -vf \"scale={width}:-2\" -q:v 2 -pix_fmt yuvj420p \
         \"{output}\"",
        "jpg",
        AddOptions {
            parameters: vec![
                // 不能叫 time：{time} 是内置占位符（当前时间 HHmmss）（源码 `SystemTemplatePresets.cs:541` 原注释）
                text_param("seek", "截取时间", "格式 时:分:秒，例如 00:00:05。", "00:00:01"),
                select_param("width", "宽度 (px)", "", "1280", vec![opt("640", "640"), opt("1280", "1280"), opt("1920", "1920")]),
            ],
            tags: vec!["封面", "缩略图"],
            ..Default::default()
        },
    );

    c.add(
        "special.faststart",
        "特殊处理",
        "网络优化 · 仅重建索引（faststart）",
        "不动画面和声音，只把索引移到文件开头，让视频可以先播后下。",
        "-i \"{input}\" -c copy -movflags +faststart \"{output}\"",
        "mp4",
        AddOptions {
            input_formats: &["mp4", "mov", "m4v"],
            tags: vec!["faststart", "网络", "无损"],
            ..Default::default()
        },
    );

    c.add(
        "special.fix_timestamps",
        "特殊处理",
        "网络优化 · 无损调整时间戳（修正音画不同步）",
        "重新生成时间戳，修复某些录制工具产生的音画不同步问题。",
        "-i \"{input}\" -c copy -fflags +genpts -avoid_negative_ts make_zero \"{output}\"",
        "mp4",
        AddOptions {
            tags: vec!["时间戳", "音画同步", "无损"],
            ..Default::default()
        },
    );

    c.add(
        "special.soft_subtitle_track",
        "特殊处理",
        "字幕 · 封装为可选字幕轨（不重新编码）",
        "把字幕作为可开关的字幕轨封装进 MP4（mov_text）。速度极快、画质无损，\
         而且不依赖 libass，任何 FFmpeg 构建都能用。播放器里可以自由开关字幕。",
        "-i \"{input}\" -i \"{subtitle}\" -map 0 -map 1:0 -c copy -c:s mov_text \
         -movflags +faststart \"{output}\"",
        "mp4",
        AddOptions {
            input_formats: &["mp4", "mkv", "mov", "m4v", "webm", "ts"],
            parameters: vec![text_param("subtitle", "字幕文件路径", "srt / ass 文件的完整路径。", "")],
            tags: vec!["字幕", "软字幕", "无损"],
            ..Default::default()
        },
    );

    c.add(
        "special.keep_all_tracks",
        "特殊处理",
        "多音轨 · 保留全部音轨与字幕",
        "把所有音轨和字幕轨一起封装到 MKV，不做任何重新编码，适合归档原始素材。",
        "-i \"{input}\" -map 0 -c copy \"{output}\"",
        "mkv",
        AddOptions {
            input_formats: &["mp4", "mkv", "mov", "avi", "ts", "webm", "flv", "m4v"],
            tags: vec!["多音轨", "归档", "无损"],
            ..Default::default()
        },
    );

    // 登记顺序最后一条，但分类是「音频处理」→ SortOrder = 50（§11 S51）
    c.add(
        "audio.copy_track",
        "音频处理",
        "音频 · 直接复制音轨（不重新编码）",
        "只把音轨取出来重新封装，不做任何编码，速度和音质都是最优。",
        "-i \"{input}\" -vn -map 0:a:0 -c:a copy \"{output}\"",
        "m4a",
        AddOptions {
            input_formats: &["mp4", "mkv", "mov", "flac", "wav", "ogg", "m4a", "aac", "ts"],
            tags: vec!["音频", "无损", "快速"],
            ..Default::default()
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 唯一允许「参数名不出现在 command_args 里」的例外：S33 `rotate.90` 的死参数 `angle`
    /// （参考实现把 `transpose=1` 写死了，见文件头与 §13.2 建议 B —— 本次要求逐字移植，故不改）。
    const DEAD_PARAMETER_ALLOWLIST: &[(&str, &str)] = &[("rotate.90", "angle")];

    /// 51 个模板：数量、id 唯一、名称唯一且非空、命令里必须有 `{input}` 与输出占位符。
    #[test]
    fn catalog_size_and_uniqueness() {
        let templates = all();
        assert_eq!(templates.len(), 51, "内置模板必须是 51 个");

        let mut ids = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        for t in &templates {
            assert!(ids.insert(t.id.clone()), "id 重复：{}", t.id);
            assert!(!t.name.trim().is_empty(), "模板名不能为空：{}", t.id);
            assert!(names.insert(t.name.clone()), "模板名重复：{}", t.name);
            assert!(t.command_args.contains("{input}"), "{} 缺少 {{input}}", t.name);
            // S45（图片序列）用 `{output_pattern}` 取代 `{output}`，是唯一例外（§11 S45）
            assert!(
                t.command_args.contains("{output}") || t.command_args.contains("{output_pattern}"),
                "{} 缺少输出占位符",
                t.name
            );
            assert_eq!(t.r#type, TemplateType::System);
            assert_eq!(t.preset_version, PRESET_VERSION);
            assert_eq!(t.id, system_template_id(&stable_key(t).unwrap()));
        }
        // 目录与 key 注册表一一对应
        assert_eq!(
            all_with_keys().iter().map(|(k, _)| k.clone()).collect::<Vec<_>>(),
            SYSTEM_KEYS.iter().map(|k| k.to_string()).collect::<Vec<_>>(),
            "SYSTEM_KEYS 必须与目录登记顺序一致"
        );
        assert_eq!(SYSTEM_KEYS.len(), 51);
    }

    /// 稳定 id：确定性 + 24 位小写十六进制 + 不同 key 不碰撞。
    #[test]
    fn system_template_id_is_deterministic_hex24() {
        let id = system_template_id("rec.mp4_h264_auto");
        assert_eq!(id, system_template_id("rec.mp4_h264_auto"));
        assert_eq!(id.len(), 24);
        assert!(
            id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "id 必须是 24 个小写十六进制字符：{id}"
        );
        // 与 models 的 ObjectId 形状一致
        assert!(crate::models::is_valid_id(&id));

        let mut seen = std::collections::HashSet::new();
        for key in SYSTEM_KEYS {
            let id = system_template_id(key);
            assert_eq!(id.len(), 24, "{key} → {id}");
            assert!(seen.insert(id), "key 哈希碰撞：{key}");
        }
        assert_eq!(seen.len(), 51);

        // 稳定 key 反查
        let templates = all();
        let t = templates.iter().find(|t| t.name == "MP4 · H.264（自动选择最佳编码器）").unwrap();
        assert_eq!(stable_key(t).as_deref(), Some("rec.mp4_h264_auto"));
        assert_eq!(key_of(t), stable_key(t));
        // 未知 id / 名称 → None（不硬凑）
        let mut unknown = t.clone();
        unknown.id = "000000000000000000000000".to_string();
        unknown.name = "不存在的模板".to_string();
        assert_eq!(stable_key(&unknown), None);
        // C# 数据文件（随机 ObjectId）按名称回退定位
        let mut legacy = t.clone();
        legacy.id = "000000000000000000000000".to_string();
        assert_eq!(stable_key(&legacy).as_deref(), Some("rec.mp4_h264_auto"));
    }

    /// 每个声明的参数名都必须出现在命令里（占位符 `{name}` 或参数组 `{name}_args`），
    /// 否则说明参数名拼错了（死参数例外见 [`DEAD_PARAMETER_ALLOWLIST`]）。
    #[test]
    fn parameter_placeholders_exist() {
        let mut violations = Vec::new();
        let mut allowlist_hits = Vec::new();
        for t in all() {
            let key = stable_key(&t).unwrap();
            for p in &t.parameters {
                let direct = t.command_args.contains(&format!("{{{}}}", p.name));
                // 参数组占位符：`{quality_args}` / `{preset_args}` 由编码器画像展开（§3.2）
                let group = t.command_args.contains(&format!("{}_args", p.name))
                    || t.command_args.contains(&format!("{{{}}}_args", p.name));
                if direct || group {
                    continue;
                }
                if DEAD_PARAMETER_ALLOWLIST.contains(&(key.as_str(), p.name.as_str())) {
                    allowlist_hits.push((key.clone(), p.name.clone()));
                } else {
                    violations.push(format!("{} :: {} :: {}", t.name, key, p.name));
                }
            }
        }
        assert!(violations.is_empty(), "参数名未出现在 command_args 中：{violations:#?}");
        // 例外表不能过期：S33 的死参数确实仍然存在
        assert_eq!(
            allowlist_hits,
            vec![("rotate.90".to_string(), "angle".to_string())],
            "死参数例外表与目录不一致"
        );
    }

    /// 分类必须都在 CATEGORY_ORDER 内（顺序与文案逐字）。
    #[test]
    fn categories_follow_category_order() {
        assert_eq!(
            CATEGORY_ORDER,
            &["推荐", "视频转码", "硬件加速", "视频压缩", "分辨率与画面", "音频处理", "特殊处理"]
        );
        for t in all() {
            assert!(
                CATEGORY_ORDER.contains(&t.category.as_str()),
                "未知分类：{}（{}）",
                t.category,
                t.name
            );
        }
        // 分类计数（§11.1）
        let count = |category: &str| all().iter().filter(|t| t.category == category).count();
        assert_eq!(count("推荐"), 4);
        assert_eq!(count("视频转码"), 6);
        assert_eq!(count("硬件加速"), 12);
        assert_eq!(count("视频压缩"), 4);
        assert_eq!(count("分辨率与画面"), 9);
        assert_eq!(count("音频处理"), 7);
        assert_eq!(count("特殊处理"), 9);
    }

    /// 10 个厂商硬件模板 + 2 个自动模板的元数据一致性。
    #[test]
    fn hardware_family_templates() {
        let templates = all();
        let by_key = |key: &str| {
            all_with_keys()
                .into_iter()
                .find(|(k, _)| k == key)
                .map(|(_, t)| t)
                .unwrap_or_else(|| panic!("缺少模板 {key}"))
        };

        // 5 个族 × 2 个编码 = 10 个 Required 模板
        for (family, display_family, vendor_description) in HARDWARE_FAMILIES {
            for (codec, tag_codec, suffix) in [("h264", "h264", "H.264"), ("hevc", "hevc", "H.265 / HEVC")] {
                let key = format!("hw.{family}.{codec}");
                let t = by_key(&key);
                assert_eq!(t.name, format!("{display_family} · {suffix}"), "{key}");
                assert_eq!(t.category, "硬件加速");
                assert_eq!(t.output_extension, "mp4");
                assert_eq!(t.target_codec, codec);
                assert_eq!(t.hardware_mode, HardwareAccelerationMode::Required);
                assert_eq!(t.required_encoder.as_deref(), Some(*family));
                assert!(t.requires_hardware_acceleration, "{key} 必须是 Required");
                assert!(!t.is_recommended, "厂商模板 recommended=false");
                assert_eq!(t.tags, vec!["硬件加速".to_string(), tag_codec.to_string(), (*family).to_string()]);
                assert!(t.supported_input_formats.is_empty());
                assert!(t.required_filters.is_empty());
                assert!(t.command_args.contains("{encoder}"));
                assert!(t.command_args.contains("-c:a {audio_encoder}"));
                assert_eq!(t.command_args.contains("-tag:v hvc1"), codec == "hevc");
                let expected_desc = if codec == "hevc" {
                    format!("使用 {vendor_description} 的 {display_family} 硬件编码 H.265，画质/体积比更好。")
                } else {
                    format!("使用 {vendor_description} 的 {display_family} 硬件编码 H.264，速度快、CPU 占用低。")
                };
                assert_eq!(t.description, expected_desc, "{key}");
                // 参数：hardQuality（20/24/28/32）+ audio_bitrate
                assert_eq!(t.parameters.len(), 2);
                assert_eq!(t.parameters[0].name, "quality");
                assert_eq!(t.parameters[0].label, "质量");
                assert_eq!(t.parameters[0].description, "留空使用该编码器的推荐默认值。");
                assert_eq!(t.parameters[0].default_value, "");
                assert_eq!(
                    t.parameters[0].options.iter().map(|o| o.value.as_str()).collect::<Vec<_>>(),
                    vec!["", "20", "24", "28", "32"]
                );
                assert_eq!(t.parameters[1].name, "audio_bitrate");
                assert_eq!(t.parameters[1].default_value, "128k");
            }
        }

        // 生成顺序：nvenc → qsv → amf → videotoolbox → vaapi，sort_order 连续 12..=21
        let hardware: Vec<_> = templates.iter().filter(|t| t.category == "硬件加速").collect();
        assert_eq!(hardware.len(), 12);
        let generated: Vec<_> = hardware.iter().filter(|t| !t.name.starts_with("硬件加速 ·")).collect();
        assert_eq!(generated.len(), 10);
        for (i, t) in generated.iter().enumerate() {
            assert_eq!(t.sort_order, 12 + i as i32, "{}", t.name);
        }
        let expected_keys: Vec<String> = HARDWARE_FAMILIES
            .iter()
            .flat_map(|(family, _, _)| [format!("hw.{family}.h264"), format!("hw.{family}.hevc")])
            .collect();
        assert_eq!(
            generated
                .iter()
                .map(|t| stable_key(t).unwrap())
                .collect::<Vec<_>>(),
            expected_keys
        );

        // 两个「自动挑选编码器」的模板：Auto + required_encoder=None
        for (key, codec) in [("hw.h264_auto", "h264"), ("hw.hevc_auto", "hevc")] {
            let t = by_key(key);
            assert_eq!(t.hardware_mode, HardwareAccelerationMode::Auto);
            assert_eq!(t.required_encoder, None);
            assert!(!t.requires_hardware_acceleration);
            assert_eq!(t.target_codec, codec);
            assert_eq!(t.category, "硬件加速");
        }
        for (key, codec, recommended) in [
            ("rec.mp4_h264_auto", "h264", true),
            ("rec.mp4_hevc_auto", "hevc", true),
        ] {
            let t = by_key(key);
            assert_eq!(t.category, "推荐");
            assert_eq!(t.hardware_mode, HardwareAccelerationMode::Auto);
            assert_eq!(t.required_encoder, None);
            assert_eq!(t.target_codec, codec);
            assert_eq!(t.is_recommended, recommended);
        }

        // 4 个缩放模板：target_codec=h264、hardware_mode=None、无参数
        for (key, name, width, height) in SCALE_TEMPLATES {
            let t = by_key(key);
            assert_eq!(t.name, *name);
            assert_eq!(t.category, "分辨率与画面");
            assert_eq!(t.target_codec, "h264");
            assert_eq!(t.hardware_mode, HardwareAccelerationMode::None);
            assert_eq!(t.required_encoder, None);
            assert!(t.parameters.is_empty());
            assert_eq!(t.tags, vec!["缩放".to_string(), format!("{height}p")]);
            let expected_args = format!(
                "-i \"{{input}}\" -vf \"scale={width}:{height}:force_original_aspect_ratio=decrease,\
                 pad={width}:{height}:(ow-iw)/2:(oh-ih)/2,setsar=1\" -c:v libx264 -crf 23 -preset medium \
                 -c:a aac -b:a 128k -movflags +faststart \"{{output}}\""
            );
            assert_eq!(t.command_args, expected_args, "{key}");
            assert_eq!(
                t.description,
                format!("等比缩放到不超过 {width}×{height}，必要时用黑边补齐，保证画面不变形。")
            );
        }
    }

    /// `sort_order` = 登记序号（0..=50），且 S51 的异常（分类「音频处理」但排最后）被保留。
    #[test]
    fn sort_order_matches_registration() {
        let templates = all();
        for (i, t) in templates.iter().enumerate() {
            assert_eq!(t.sort_order, i as i32, "{}", t.name);
        }
        let last = templates.last().unwrap();
        assert_eq!(last.name, "音频 · 直接复制音轨（不重新编码）");
        assert_eq!(last.category, "音频处理");
        assert_eq!(last.sort_order, 50);
        // all() 的排序契约
        let mut sorted = all();
        sorted.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then_with(|| a.name.cmp(&b.name)));
        assert_eq!(
            sorted.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
            templates.iter().map(|t| t.id.clone()).collect::<Vec<_>>()
        );
    }

    /// 参考实现的已知缺陷必须**原样保留**（防止后续「顺手修复」）：
    /// S33 死参数、S45 `{output_pattern}`、S46 `seek`、S43 单引号路径。
    #[test]
    fn known_quirks_are_preserved() {
        let by_key = |key: &str| {
            all_with_keys()
                .into_iter()
                .find(|(k, _)| k == key)
                .map(|(_, t)| t)
                .unwrap()
        };
        assert!(by_key("rotate.90").command_args.contains("-vf \"transpose=1\""));
        assert_eq!(by_key("rotate.90").parameters[0].name, "angle");
        assert!(by_key("special.image_sequence").command_args.ends_with("\"{output_pattern}\""));
        let thumbnail = by_key("special.thumbnail");
        assert!(thumbnail.command_args.starts_with("-ss {seek} -i \"{input}\""));
        assert!(!thumbnail.command_args.contains("{time}"));
        assert!(by_key("special.burn_subtitles").command_args.contains("filename='{subtitle}'"));
        assert_eq!(by_key("special.burn_subtitles").required_filters, vec!["subtitles".to_string()]);
        // 混用半角括号的名称保持原样
        assert!(by_key("video.mp4_x264").name.ends_with("x264)"));
    }

    /// 黄金文件：完整目录（名称 / 描述 / 命令 / 参数）逐行比对。
    ///
    /// 重新生成（内容有意变更时才做，并请在 review 里逐行确认差异）：
    ///
    /// ```text
    /// UPDATE_PRESETS_GOLDEN=1 cargo test templates::presets::tests::golden_catalog
    /// ```
    #[test]
    fn golden_catalog() {
        let actual = golden_dump();
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/templates/presets.golden.txt");
        if std::env::var_os("UPDATE_PRESETS_GOLDEN").is_some() {
            std::fs::write(path, &actual).expect("写入黄金文件失败");
            return;
        }
        let expected = std::fs::read_to_string(path).unwrap_or_else(|err| {
            panic!("读取黄金文件 {path} 失败：{err}（用 UPDATE_PRESETS_GOLDEN=1 cargo test 生成）")
        });
        assert_eq!(
            actual, expected,
            "内置模板与黄金文件不一致：若是有意修改，请重新生成并逐行 review"
        );
    }

    fn kind_name(kind: TemplateParameterKind) -> &'static str {
        match kind {
            TemplateParameterKind::Select => "Select",
            TemplateParameterKind::Number => "Number",
            TemplateParameterKind::Text => "Text",
            TemplateParameterKind::Boolean => "Boolean",
        }
    }

    fn hardware_mode_name(mode: HardwareAccelerationMode) -> &'static str {
        match mode {
            HardwareAccelerationMode::None => "None",
            HardwareAccelerationMode::Auto => "Auto",
            HardwareAccelerationMode::Required => "Required",
        }
    }

    fn join(values: &[String], sep: &str) -> String {
        values.join(sep)
    }

    fn golden_dump() -> String {
        let mut out = String::new();
        out.push_str("# 内置模板预设 golden 目录（逐字移植自 SystemTemplatePresets.cs，规格 docs/reference/templates.md §11）\n");
        out.push_str("# 重新生成：UPDATE_PRESETS_GOLDEN=1 cargo test templates::presets::tests::golden_catalog\n");
        out.push_str("# 不含时间戳（created_at/updated_at 每次构造都不同）；id 由稳定 key 派生，因此可以纳入比对。\n");
        out.push_str(&format!("preset_version={PRESET_VERSION}\n"));
        out.push_str(&format!("categories={}\n", CATEGORY_ORDER.join("|")));
        out.push_str(&format!("count={}\n", all().len()));

        for (index, t) in all().into_iter().enumerate() {
            let key = stable_key(&t).unwrap_or_default();
            out.push_str("---\n");
            out.push_str(&format!(
                "[{index:02}] key={key} id={} sort_order={} category={} type=System name={}\n",
                t.id,
                t.sort_order,
                t.category,
                t.name
            ));
            out.push_str(&format!(
                "  flags=recommended:{} output_extension={} target_codec={} hardware_mode={} required_encoder={} \
                 requires_hardware_acceleration={}\n",
                t.is_recommended,
                t.output_extension,
                if t.target_codec.is_empty() { "-" } else { t.target_codec.as_str() },
                hardware_mode_name(t.hardware_mode),
                t.required_encoder.as_deref().unwrap_or("-"),
                t.requires_hardware_acceleration
            ));
            out.push_str(&format!(
                "  input_formats=[{}] required_filters=[{}] tags=[{}]\n",
                join(&t.supported_input_formats, ", "),
                join(&t.required_filters, ", "),
                join(&t.tags, ", ")
            ));
            out.push_str(&format!("  description={}\n", t.description));
            out.push_str(&format!("  command_args={}\n", t.command_args));

            // 参数按 name 升序（确定性；声明顺序由 §11 与源码负责）
            let mut params = t.parameters.clone();
            params.sort_by(|a, b| a.name.cmp(&b.name));
            for p in params {
                let options = p
                    .options
                    .iter()
                    .map(|o| format!("\"{}\"→\"{}\"", o.value, o.label))
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push_str(&format!(
                    "  param {} | label={} | kind={} | default={} | min={} | max={} | true_value={} | false_value={} \
                     | description={} | options=[{}]\n",
                    p.name,
                    p.label,
                    kind_name(p.kind),
                    p.default_value,
                    p.min.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string()),
                    p.max.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string()),
                    p.true_value,
                    p.false_value,
                    p.description,
                    options
                ));
            }
        }
        out
    }
}
