//! 编码器画像（规格 `ffmpeg-core.md` §6 / `templates.md` §5.1，源 `Services/EncoderCatalog.cs`）。
//!
//! 31 条画像**逐字照搬**源码，`{q}` 占位符在渲染时替换为质量数值。
//! 这是模板里 `{quality_args}` / `{preset_args}` / `{hwupload_args}` / `{device_args}` /
//! `{muxer_args}` 的真正来源，也是硬件检测「试跑」用的参数集。

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

/// 硬件编码器厂商/类型（全项目唯一定义）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum EncoderType {
    /// 软件编码（CPU）
    #[default]
    Software = 0,
    /// NVIDIA NVENC
    Nvenc = 1,
    /// Intel Quick Sync Video
    Qsv = 2,
    /// AMD Advanced Media Framework
    Amf = 3,
    /// Apple VideoToolbox
    VideoToolbox = 4,
    /// Linux VA-API（Intel/AMD 通用）
    Vaapi = 5,
    /// Vulkan Video（跨厂商）
    Vulkan = 6,
    /// Windows Media Foundation
    MediaFoundation = 7,
    /// 其他/未知
    Other = 99,
}

impl EncoderType {
    pub const ALL: &'static [EncoderType] = &[
        EncoderType::Software,
        EncoderType::Nvenc,
        EncoderType::Qsv,
        EncoderType::Amf,
        EncoderType::VideoToolbox,
        EncoderType::Vaapi,
        EncoderType::Vulkan,
        EncoderType::MediaFoundation,
        EncoderType::Other,
    ];

    /// 族标识（与画像表的 `Family` 列一致）
    pub fn id(&self) -> &'static str {
        match self {
            EncoderType::Software => "software",
            EncoderType::Nvenc => "nvenc",
            EncoderType::Qsv => "qsv",
            EncoderType::Amf => "amf",
            EncoderType::VideoToolbox => "videotoolbox",
            EncoderType::Vaapi => "vaapi",
            EncoderType::Vulkan => "vulkan",
            EncoderType::MediaFoundation => "mf",
            EncoderType::Other => "other",
        }
    }

    /// 中文显示名（文案取自 `Models/Enums.cs` 的注释）
    pub fn display_name(&self) -> &'static str {
        match self {
            EncoderType::Software => "软件编码（CPU）",
            EncoderType::Nvenc => "NVIDIA NVENC",
            EncoderType::Qsv => "Intel Quick Sync Video",
            EncoderType::Amf => "AMD AMF",
            EncoderType::VideoToolbox => "Apple VideoToolbox",
            EncoderType::Vaapi => "Linux VA-API",
            EncoderType::Vulkan => "Vulkan Video",
            EncoderType::MediaFoundation => "Windows Media Foundation",
            EncoderType::Other => "其它",
        }
    }

    pub fn from_id(value: &str) -> Option<EncoderType> {
        EncoderType::ALL.iter().copied().find(|t| t.id().eq_ignore_ascii_case(value))
    }

    /// 字符串是否属于本类型（用于模板里写 `"nvenc"` 这类族名）。
    ///
    /// `Other` 不匹配任何族名（它只是「未知」的兜底）。
    pub fn family_matches(&self, value: &str) -> bool {
        let value = value.trim();
        if value.is_empty() {
            return false;
        }
        match self {
            EncoderType::Other => false,
            _ => self.id().eq_ignore_ascii_case(value) || self.display_name().eq_ignore_ascii_case(value),
        }
    }

    /// 数据库/导出 JSON 使用的整数值
    pub fn to_int(self) -> i64 {
        self as i64
    }

    pub fn from_int(value: i64) -> Self {
        match value {
            0 => EncoderType::Software,
            1 => EncoderType::Nvenc,
            2 => EncoderType::Qsv,
            3 => EncoderType::Amf,
            4 => EncoderType::VideoToolbox,
            5 => EncoderType::Vaapi,
            6 => EncoderType::Vulkan,
            7 => EncoderType::MediaFoundation,
            99 => EncoderType::Other,
            _ => EncoderType::Other,
        }
    }
}

/// 单个编码器的参数画像（对应 C# `EncoderProfile` record）。
///
/// `quality_args` / `preset_args` / `upload_filter` / `device_args` / `muxer_args` 都是
/// **逐字的命令行片段**（含 `{q}` 占位符），空串表示「该编码器没有这一项」——
/// 与 C# 里 `null` 的语义等价。
#[derive(Debug, Clone, Serialize)]
pub struct EncoderProfile {
    /// FFmpeg 编码器名，如 `h264_nvenc`
    pub name: String,
    /// 界面显示名
    pub display_name: String,
    /// 所属厂商/类型
    pub encoder_type: EncoderType,
    /// 族标识：`nvenc` / `qsv` / `amf` / `videotoolbox` / `vaapi` / `vulkan` / `mf` / `software`
    pub family: String,
    /// 目标编码格式：`h264` / `hevc` / `av1` / `vp9` / `vp8` / `prores` / `aac` / `mp3` / `opus` / `flac`
    pub codec: String,
    /// 质量参数，逐字来自画像表，含 `{q}` 占位（如 `-rc vbr -cq {q} -b:v 0`）；空串 = 不接受质量参数
    pub quality_args: String,
    /// 默认质量数值（C# 默认 23）
    pub default_quality: i32,
    /// 质量范围提示文案（**仅提示**，不做任何数值缩放/钳制）
    pub quality_hint: String,
    /// 预设参数，逐字（如 `-preset p5 -tune hq`）；空串 = 无预设
    pub preset_args: String,
    /// 送入编码器前必须附加的滤镜（如 `format=nv12,hwupload`）；空串 = 可直接吃系统内存帧
    pub upload_filter: String,
    /// 设备初始化参数（如 `-vaapi_device /dev/dri/renderD128`）；空串 = 无
    pub device_args: String,
    /// 输出容器附加参数（如 `-tag:v hvc1`）；空串 = 无
    pub muxer_args: String,
    /// 同编码格式内优先级，越小越优先
    pub sort_preference: i32,
    /// 允许的平台；**空数组 = 全平台**（对应 C# 未设置 `SupportsCurrentPlatform`）
    pub platforms: &'static [&'static str],
}

impl Default for EncoderProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            display_name: String::new(),
            encoder_type: EncoderType::Software,
            family: "software".to_string(),
            codec: String::new(),
            quality_args: String::new(),
            // C# record 默认值：DefaultQuality = 23
            default_quality: 23,
            // C# record 默认值：QualityHint = "数值越小画质越好"
            quality_hint: "数值越小画质越好".to_string(),
            preset_args: String::new(),
            upload_filter: String::new(),
            device_args: String::new(),
            muxer_args: String::new(),
            sort_preference: 0,
            platforms: &[],
        }
    }
}

impl EncoderProfile {
    /// 把 `{q}` 替换成质量值（`quality` 为 `None` 时用 `default_quality`）。
    ///
    /// 质量值是**直通**的整数：不做范围钳制、不做缩放、不做逆向（规格 §6.3）。
    pub fn render_quality_args(&self, quality: Option<i32>) -> String {
        let q = quality.unwrap_or(self.default_quality);
        self.quality_args.replace("{q}", &q.to_string())
    }

    /// 展开成分词后的质量参数（供试跑与模板引擎使用）。
    pub fn quality_args_tokens(&self, quality: Option<i32>) -> Vec<String> {
        if self.quality_args.trim().is_empty() {
            return Vec::new();
        }
        crate::ffmpeg::util::tokenize(&self.render_quality_args(quality))
    }

    /// 展开成分词后的预设参数（失败时返回空）。
    pub fn preset_args_tokens(&self) -> Vec<String> {
        if self.preset_args.trim().is_empty() {
            return Vec::new();
        }
        crate::ffmpeg::util::tokenize(&self.preset_args)
    }

    /// 展开成分词后的设备参数。
    pub fn device_args_tokens(&self) -> Vec<String> {
        if self.device_args.trim().is_empty() {
            return Vec::new();
        }
        crate::ffmpeg::util::tokenize(&self.device_args)
    }

    /// 展开成分词后的容器参数。
    pub fn muxer_args_tokens(&self) -> Vec<String> {
        if self.muxer_args.trim().is_empty() {
            return Vec::new();
        }
        crate::ffmpeg::util::tokenize(&self.muxer_args)
    }

    /// 是否硬件编码器（`Type != Software`）
    pub fn is_hardware(&self) -> bool {
        self.encoder_type != EncoderType::Software
    }

    /// 该画像是否存在于当前平台（`platforms` 为空视为全平台）。
    pub fn is_platform_supported(&self) -> bool {
        self.platforms.is_empty() || self.platforms.contains(&crate::paths::platform())
    }

    /// 该画像是否显式声明支持指定平台（不做「空 = 全平台」推断）。
    pub fn supports_platform(&self, platform: &str) -> bool {
        self.platforms.is_empty() || self.platforms.iter().any(|p| p.eq_ignore_ascii_case(platform))
    }
}

// 平台限制常量（逐字来自画像表的 `SupportsCurrentPlatform`）
const WIN_LINUX: &[&str] = &["windows", "linux"];
const MACOS_ONLY: &[&str] = &["macos"];
const LINUX_ONLY: &[&str] = &["linux"];
const WINDOWS_ONLY: &[&str] = &["windows"];
/// VAAPI 系列固定的设备参数（全表唯一的 `DeviceArgs` 值）
const VAAPI_DEVICE: &str = "-vaapi_device /dev/dri/renderD128";
/// VAAPI 系列固定的上传滤镜
const VAAPI_UPLOAD: &str = "format=nv12,hwupload";
/// HEVC 画像统一的容器参数（MP4/MOV 兼容）
const HVC1_TAG: &str = "-tag:v hvc1";

/// 构建 31 条画像（顺序与源码一致；顺序也是同 `sort_preference` 时的稳定次序）。
fn build_profiles() -> Vec<EncoderProfile> {
    let mut out = Vec::with_capacity(31);

    // ── NVIDIA NVENC（Windows + Linux）──────────────────────────────────
    out.push(EncoderProfile {
        name: "h264_nvenc".into(),
        display_name: "NVIDIA NVENC H.264".into(),
        encoder_type: EncoderType::Nvenc,
        family: "nvenc".into(),
        codec: "h264".into(),
        quality_args: "-rc vbr -cq {q} -b:v 0".into(),
        default_quality: 24,
        quality_hint: "CQ 值，18(高)~32(小体积)".into(),
        preset_args: "-preset p5 -tune hq".into(),
        sort_preference: 10,
        platforms: WIN_LINUX,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "hevc_nvenc".into(),
        display_name: "NVIDIA NVENC H.265/HEVC".into(),
        encoder_type: EncoderType::Nvenc,
        family: "nvenc".into(),
        codec: "hevc".into(),
        quality_args: "-rc vbr -cq {q} -b:v 0".into(),
        default_quality: 28,
        quality_hint: "CQ 值，22(高)~35(小体积)".into(),
        preset_args: "-preset p5 -tune hq".into(),
        muxer_args: HVC1_TAG.into(),
        sort_preference: 10,
        platforms: WIN_LINUX,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "av1_nvenc".into(),
        display_name: "NVIDIA NVENC AV1".into(),
        encoder_type: EncoderType::Nvenc,
        family: "nvenc".into(),
        codec: "av1".into(),
        quality_args: "-rc vbr -cq {q} -b:v 0".into(),
        default_quality: 30,
        quality_hint: "CQ 值，24(高)~38(小体积)".into(),
        preset_args: "-preset p5 -tune hq".into(),
        sort_preference: 10,
        platforms: WIN_LINUX,
        ..Default::default()
    });

    // ── Intel Quick Sync Video（Windows + Linux）────────────────────────
    out.push(EncoderProfile {
        name: "h264_qsv".into(),
        display_name: "Intel Quick Sync H.264".into(),
        encoder_type: EncoderType::Qsv,
        family: "qsv".into(),
        codec: "h264".into(),
        quality_args: "-global_quality {q}".into(),
        default_quality: 24,
        quality_hint: "ICQ 值，18(高)~32(小体积)".into(),
        preset_args: "-preset medium".into(),
        sort_preference: 20,
        platforms: WIN_LINUX,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "hevc_qsv".into(),
        display_name: "Intel Quick Sync H.265/HEVC".into(),
        encoder_type: EncoderType::Qsv,
        family: "qsv".into(),
        codec: "hevc".into(),
        quality_args: "-global_quality {q}".into(),
        default_quality: 28,
        quality_hint: "ICQ 值，22(高)~35(小体积)".into(),
        preset_args: "-preset medium".into(),
        muxer_args: HVC1_TAG.into(),
        sort_preference: 20,
        platforms: WIN_LINUX,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "av1_qsv".into(),
        display_name: "Intel Quick Sync AV1".into(),
        encoder_type: EncoderType::Qsv,
        family: "qsv".into(),
        codec: "av1".into(),
        quality_args: "-global_quality {q}".into(),
        default_quality: 30,
        quality_hint: "ICQ 值，24(高)~38(小体积)".into(),
        preset_args: "-preset medium".into(),
        sort_preference: 20,
        platforms: WIN_LINUX,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "vp9_qsv".into(),
        display_name: "Intel Quick Sync VP9".into(),
        encoder_type: EncoderType::Qsv,
        family: "qsv".into(),
        codec: "vp9".into(),
        quality_args: "-global_quality {q}".into(),
        default_quality: 30,
        preset_args: "-preset medium".into(),
        sort_preference: 20,
        platforms: WIN_LINUX,
        ..Default::default()
    });

    // ── AMD AMF（Windows + Linux）──────────────────────────────────────
    out.push(EncoderProfile {
        name: "h264_amf".into(),
        display_name: "AMD AMF H.264".into(),
        encoder_type: EncoderType::Amf,
        family: "amf".into(),
        codec: "h264".into(),
        quality_args: "-rc cqp -qp_i {q} -qp_p {q}".into(),
        default_quality: 24,
        quality_hint: "QP 值，18(高)~32(小体积)".into(),
        preset_args: "-quality balanced".into(),
        sort_preference: 30,
        platforms: WIN_LINUX,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "hevc_amf".into(),
        display_name: "AMD AMF H.265/HEVC".into(),
        encoder_type: EncoderType::Amf,
        family: "amf".into(),
        codec: "hevc".into(),
        quality_args: "-rc cqp -qp_i {q} -qp_p {q}".into(),
        default_quality: 28,
        quality_hint: "QP 值，22(高)~35(小体积)".into(),
        preset_args: "-quality balanced".into(),
        muxer_args: HVC1_TAG.into(),
        sort_preference: 30,
        platforms: WIN_LINUX,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "av1_amf".into(),
        display_name: "AMD AMF AV1".into(),
        encoder_type: EncoderType::Amf,
        family: "amf".into(),
        codec: "av1".into(),
        quality_args: "-rc cqp -qp_i {q} -qp_p {q}".into(),
        default_quality: 30,
        preset_args: "-quality balanced".into(),
        sort_preference: 30,
        platforms: WIN_LINUX,
        ..Default::default()
    });

    // ── Apple VideoToolbox（仅 macOS）───────────────────────────────────
    out.push(EncoderProfile {
        name: "h264_videotoolbox".into(),
        display_name: "Apple VideoToolbox H.264".into(),
        encoder_type: EncoderType::VideoToolbox,
        family: "videotoolbox".into(),
        codec: "h264".into(),
        quality_args: "-q:v {q}".into(),
        default_quality: 60,
        // PORT NOTE：VideoToolbox 的质量方向与其它厂商相反（0~100 越大越好），
        // 参考实现不做任何换算，只给这一句文案（规格 §8.9）。
        quality_hint: "质量 0~100，越大画质越好".into(),
        sort_preference: 5,
        platforms: MACOS_ONLY,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "hevc_videotoolbox".into(),
        display_name: "Apple VideoToolbox H.265/HEVC".into(),
        encoder_type: EncoderType::VideoToolbox,
        family: "videotoolbox".into(),
        codec: "hevc".into(),
        quality_args: "-q:v {q}".into(),
        default_quality: 60,
        quality_hint: "质量 0~100，越大画质越好".into(),
        muxer_args: HVC1_TAG.into(),
        sort_preference: 5,
        platforms: MACOS_ONLY,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "prores_videotoolbox".into(),
        display_name: "Apple VideoToolbox ProRes".into(),
        encoder_type: EncoderType::VideoToolbox,
        family: "videotoolbox".into(),
        codec: "prores".into(),
        // 无质量参数（源码里 QualityArgs = null），默认质量仍是 record 默认的 23
        quality_args: String::new(),
        quality_hint: "数值越小画质越好".into(),
        preset_args: "-profile:v 3".into(),
        // PORT NOTE：`ffmpeg-core.md` §6.2 把本行 SortPreference 写作 0，
        // 但 C# 源码（`EncoderCatalog.cs:175`）与 `templates.md` §5.1 都是 5，此处取 5。
        sort_preference: 5,
        platforms: MACOS_ONLY,
        ..Default::default()
    });

    // ── Linux VA-API（仅 Linux；`DeviceArgs` 全表唯一）──────────────────
    out.push(EncoderProfile {
        name: "h264_vaapi".into(),
        display_name: "VA-API H.264 (Intel/AMD)".into(),
        encoder_type: EncoderType::Vaapi,
        family: "vaapi".into(),
        codec: "h264".into(),
        quality_args: "-qp {q}".into(),
        default_quality: 24,
        quality_hint: "QP 值，18(高)~32(小体积)".into(),
        upload_filter: VAAPI_UPLOAD.into(),
        device_args: VAAPI_DEVICE.into(),
        sort_preference: 15,
        platforms: LINUX_ONLY,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "hevc_vaapi".into(),
        display_name: "VA-API H.265/HEVC (Intel/AMD)".into(),
        encoder_type: EncoderType::Vaapi,
        family: "vaapi".into(),
        codec: "hevc".into(),
        quality_args: "-qp {q}".into(),
        default_quality: 28,
        quality_hint: "QP 值，22(高)~35(小体积)".into(),
        upload_filter: VAAPI_UPLOAD.into(),
        device_args: VAAPI_DEVICE.into(),
        muxer_args: HVC1_TAG.into(),
        sort_preference: 15,
        platforms: LINUX_ONLY,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "av1_vaapi".into(),
        display_name: "VA-API AV1 (Intel/AMD)".into(),
        encoder_type: EncoderType::Vaapi,
        family: "vaapi".into(),
        codec: "av1".into(),
        quality_args: "-qp {q}".into(),
        default_quality: 30,
        upload_filter: VAAPI_UPLOAD.into(),
        device_args: VAAPI_DEVICE.into(),
        sort_preference: 15,
        platforms: LINUX_ONLY,
        ..Default::default()
    });

    // ── Vulkan Video（Windows + Linux，FFmpeg 8+）──────────────────────
    out.push(EncoderProfile {
        name: "h264_vulkan".into(),
        display_name: "Vulkan Video H.264".into(),
        encoder_type: EncoderType::Vulkan,
        family: "vulkan".into(),
        codec: "h264".into(),
        quality_args: "-qp {q}".into(),
        default_quality: 24,
        sort_preference: 40,
        platforms: WIN_LINUX,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "hevc_vulkan".into(),
        display_name: "Vulkan Video H.265/HEVC".into(),
        encoder_type: EncoderType::Vulkan,
        family: "vulkan".into(),
        codec: "hevc".into(),
        quality_args: "-qp {q}".into(),
        default_quality: 28,
        muxer_args: HVC1_TAG.into(),
        sort_preference: 40,
        platforms: WIN_LINUX,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "av1_vulkan".into(),
        display_name: "Vulkan Video AV1".into(),
        encoder_type: EncoderType::Vulkan,
        family: "vulkan".into(),
        codec: "av1".into(),
        quality_args: "-qp {q}".into(),
        default_quality: 30,
        sort_preference: 40,
        platforms: WIN_LINUX,
        ..Default::default()
    });

    // ── Windows Media Foundation（仅 Windows）──────────────────────────
    out.push(EncoderProfile {
        name: "h264_mf".into(),
        display_name: "Media Foundation H.264".into(),
        encoder_type: EncoderType::MediaFoundation,
        family: "mf".into(),
        codec: "h264".into(),
        quality_args: "-rate_control quality -quality {q}".into(),
        default_quality: 70,
        sort_preference: 60,
        platforms: WINDOWS_ONLY,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "hevc_mf".into(),
        display_name: "Media Foundation H.265/HEVC".into(),
        encoder_type: EncoderType::MediaFoundation,
        family: "mf".into(),
        codec: "hevc".into(),
        quality_args: "-rate_control quality -quality {q}".into(),
        default_quality: 70,
        muxer_args: HVC1_TAG.into(),
        sort_preference: 60,
        platforms: WINDOWS_ONLY,
        ..Default::default()
    });

    // ── 软件编码（始终全平台可用，作为兜底）────────────────────────────
    // 源码未设置 `SupportsCurrentPlatform` → 取默认 true → 这里对应 `platforms: &[]`。
    out.push(EncoderProfile {
        name: "libx264".into(),
        display_name: "软件编码 H.264 (x264)".into(),
        encoder_type: EncoderType::Software,
        family: "software".into(),
        codec: "h264".into(),
        quality_args: "-crf {q}".into(),
        default_quality: 23,
        quality_hint: "CRF 值，18(高)~32(小体积)".into(),
        preset_args: "-preset medium".into(),
        sort_preference: 90,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "libx265".into(),
        display_name: "软件编码 H.265 (x265)".into(),
        encoder_type: EncoderType::Software,
        family: "software".into(),
        codec: "hevc".into(),
        quality_args: "-crf {q}".into(),
        default_quality: 28,
        quality_hint: "CRF 值，22(高)~35(小体积)".into(),
        preset_args: "-preset medium".into(),
        muxer_args: HVC1_TAG.into(),
        sort_preference: 90,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "libsvtav1".into(),
        display_name: "软件编码 AV1 (SVT-AV1)".into(),
        encoder_type: EncoderType::Software,
        family: "software".into(),
        codec: "av1".into(),
        quality_args: "-crf {q} -b:v 0".into(),
        default_quality: 32,
        quality_hint: "CRF 值，24(高)~42(小体积)".into(),
        preset_args: "-preset 8".into(),
        sort_preference: 90,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "libaom-av1".into(),
        display_name: "软件编码 AV1 (libaom)".into(),
        encoder_type: EncoderType::Software,
        family: "software".into(),
        codec: "av1".into(),
        quality_args: "-crf {q} -b:v 0".into(),
        default_quality: 32,
        preset_args: "-cpu-used 4".into(),
        sort_preference: 95,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "libvpx-vp9".into(),
        display_name: "软件编码 VP9 (libvpx)".into(),
        encoder_type: EncoderType::Software,
        family: "software".into(),
        codec: "vp9".into(),
        quality_args: "-crf {q} -b:v 0".into(),
        default_quality: 32,
        preset_args: "-row-mt 1".into(),
        sort_preference: 90,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "libvpx".into(),
        display_name: "软件编码 VP8 (libvpx)".into(),
        encoder_type: EncoderType::Software,
        family: "software".into(),
        codec: "vp8".into(),
        quality_args: "-crf {q} -b:v 1M".into(),
        default_quality: 10,
        sort_preference: 90,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "aac".into(),
        display_name: "AAC 音频 (内置)".into(),
        encoder_type: EncoderType::Software,
        family: "software".into(),
        codec: "aac".into(),
        quality_args: "-b:a {q}k".into(),
        default_quality: 192,
        sort_preference: 90,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "libmp3lame".into(),
        display_name: "MP3 音频 (LAME)".into(),
        encoder_type: EncoderType::Software,
        family: "software".into(),
        codec: "mp3".into(),
        quality_args: "-b:a {q}k".into(),
        default_quality: 192,
        sort_preference: 90,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "libopus".into(),
        display_name: "Opus 音频".into(),
        encoder_type: EncoderType::Software,
        family: "software".into(),
        codec: "opus".into(),
        quality_args: "-b:a {q}k".into(),
        default_quality: 128,
        sort_preference: 90,
        ..Default::default()
    });
    out.push(EncoderProfile {
        name: "flac".into(),
        display_name: "FLAC 无损音频".into(),
        encoder_type: EncoderType::Software,
        family: "software".into(),
        codec: "flac".into(),
        // 无质量参数（源码里 QualityArgs = null）
        quality_args: String::new(),
        quality_hint: "数值越小画质越好".into(),
        sort_preference: 90,
        ..Default::default()
    });

    out
}

static PROFILES: LazyLock<Vec<EncoderProfile>> = LazyLock::new(build_profiles);

/// 编码器目录（对应 C# 静态类 `EncoderCatalog`）。
pub struct EncoderCatalog;

impl EncoderCatalog {
    /// 全部 31 条画像，顺序与源码一致。
    pub fn all() -> &'static [EncoderProfile] {
        PROFILES.as_slice()
    }

    /// 按编码器名查找（**忽略大小写**），无则 `None`。
    pub fn find(id: &str) -> Option<&'static EncoderProfile> {
        let trimmed = id.trim();
        if trimmed.is_empty() {
            return None;
        }
        Self::all().iter().find(|p| p.name.eq_ignore_ascii_case(trimmed))
    }

    /// `find` 的别名（API/模板层按需取画像）。
    pub fn resolve_profile(encoder_name: &str) -> Option<&'static EncoderProfile> {
        Self::find(encoder_name)
    }

    /// 按族名列出全部画像（保持源码顺序）。
    pub fn by_family(family: &str) -> Vec<&'static EncoderProfile> {
        let family = family.trim();
        if family.is_empty() {
            return Vec::new();
        }
        Self::all().iter().filter(|p| p.family.eq_ignore_ascii_case(family)).collect()
    }

    /// 按目标编码格式列出候选（**平台过滤 + `sort_preference` 升序**，同值保持源码顺序）。
    pub fn for_codec(codec: &str) -> Vec<&'static EncoderProfile> {
        let codec = codec.trim();
        if codec.is_empty() {
            return Vec::new();
        }
        let mut list: Vec<&'static EncoderProfile> = Self::all()
            .iter()
            .filter(|p| p.codec.eq_ignore_ascii_case(codec) && p.is_platform_supported())
            .collect();
        list.sort_by_key(|p| p.sort_preference); // Rust 的 sort_by_key 是稳定排序
        list
    }

    /// 某个编码格式对应的软件编码器（`sort_preference` 最小者）。
    pub fn software_encoder_for(codec: &str) -> Option<&'static EncoderProfile> {
        let codec = codec.trim();
        if codec.is_empty() {
            return None;
        }
        Self::all()
            .iter()
            .filter(|p| p.encoder_type == EncoderType::Software && p.codec.eq_ignore_ascii_case(codec))
            .min_by_key(|p| p.sort_preference)
    }

    /// 把族名或编码器名规范化成「一组成员名」：族名 → 该族成员（按目录顺序）；否则 `[原值]`。
    pub fn expand_family(name_or_family: &str) -> Vec<String> {
        let trimmed = name_or_family.trim();
        let members: Vec<String> =
            Self::by_family(trimmed).into_iter().map(|p| p.name.clone()).collect();
        if !members.is_empty() {
            return members;
        }
        if trimmed.is_empty() {
            return Vec::new();
        }
        vec![trimmed.to_string()]
    }

    /// 族的显示名（`nvenc` → `NVIDIA NVENC`；未知族原样返回）。
    pub fn family_display_name(family: &str) -> String {
        match family.trim().to_ascii_lowercase().as_str() {
            "nvenc" => "NVIDIA NVENC".to_string(),
            "qsv" => "Intel Quick Sync".to_string(),
            "amf" => "AMD AMF".to_string(),
            "videotoolbox" => "Apple VideoToolbox".to_string(),
            "vaapi" => "Linux VA-API".to_string(),
            "vulkan" => "Vulkan Video".to_string(),
            "mf" => "Windows Media Foundation".to_string(),
            "software" => "软件编码".to_string(),
            _ => family.trim().to_string(),
        }
    }

    /// 当前平台：`macos` / `windows` / `linux`（其它一律按 Linux 处理，规格 §1.1）。
    pub fn platform() -> &'static str {
        crate::paths::platform()
    }

    /// 目标编码格式归一（规格 §5.5.1 / `HardwareDetectionService.cs:247-264`）。
    pub fn normalize_codec(codec: &str) -> String {
        let lower = codec.trim().to_ascii_lowercase();
        match lower.as_str() {
            "avc" | "h264" | "x264" | "avc1" => "h264".to_string(),
            "h265" | "hevc" | "x265" | "hvc1" | "hev1" => "hevc".to_string(),
            "av1" | "av01" => "av1".to_string(),
            "vp9" | "vp09" => "vp9".to_string(),
            "vp8" => "vp8".to_string(),
            "prores" => "prores".to_string(),
            "aac" => "aac".to_string(),
            "mp3" => "mp3".to_string(),
            "opus" => "opus".to_string(),
            "flac" => "flac".to_string(),
            "" => String::new(),
            _ => lower,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_31_profiles_in_source_order() {
        let all = EncoderCatalog::all();
        assert_eq!(all.len(), 31);
        assert_eq!(all[0].name, "h264_nvenc");
        assert_eq!(all[30].name, "flac");
    }

    #[test]
    fn find_is_case_insensitive() {
        let profile = EncoderCatalog::find("H264_NVENC").expect("h264_nvenc 应当在画像表里");
        assert_eq!(profile.encoder_type, EncoderType::Nvenc);
        assert_eq!(profile.default_quality, 24);
        assert_eq!(profile.quality_args, "-rc vbr -cq {q} -b:v 0");
        assert!(EncoderCatalog::find("no_such_encoder").is_none());
    }

    #[test]
    fn render_quality_args_uses_default_and_override() {
        let libx264 = EncoderCatalog::find("libx264").expect("libx264");
        assert_eq!(libx264.render_quality_args(None), "-crf 23");
        assert_eq!(libx264.render_quality_args(Some(18)), "-crf 18");
        assert_eq!(libx264.quality_args_tokens(Some(18)), vec!["-crf", "18"]);

        let flac = EncoderCatalog::find("flac").expect("flac");
        assert_eq!(flac.render_quality_args(Some(99)), "");
        assert!(flac.quality_args_tokens(Some(99)).is_empty());

        let nvenc = EncoderCatalog::find("h264_nvenc").expect("h264_nvenc");
        assert_eq!(
            nvenc.quality_args_tokens(None),
            vec!["-rc", "vbr", "-cq", "24", "-b:v", "0"]
        );
        assert_eq!(nvenc.preset_args_tokens(), vec!["-preset", "p5", "-tune", "hq"]);
    }

    #[test]
    fn for_codec_sorts_by_preference_and_filters_platform() {
        let h264 = EncoderCatalog::for_codec("h264");
        assert!(h264.windows(2).all(|w| w[0].sort_preference <= w[1].sort_preference));
        assert!(h264.iter().all(|p| p.is_platform_supported()));

        if cfg!(target_os = "macos") {
            let names: Vec<&str> = h264.iter().map(|p| p.name.as_str()).collect();
            assert_eq!(names, vec!["h264_videotoolbox", "libx264"]);
        }
        assert!(EncoderCatalog::for_codec("").is_empty());
    }

    #[test]
    fn expand_family_and_display_name() {
        assert_eq!(
            EncoderCatalog::expand_family("nvenc"),
            vec!["h264_nvenc", "hevc_nvenc", "av1_nvenc"]
        );
        assert_eq!(EncoderCatalog::expand_family("hevc_nvenc"), vec!["hevc_nvenc"]);
        assert_eq!(EncoderCatalog::family_display_name("qsv"), "Intel Quick Sync");
        assert_eq!(EncoderCatalog::family_display_name("unknown"), "unknown");
    }

    #[test]
    fn normalize_codec_matches_spec() {
        assert_eq!(EncoderCatalog::normalize_codec(" AVC1 "), "h264");
        assert_eq!(EncoderCatalog::normalize_codec("H265"), "hevc");
        assert_eq!(EncoderCatalog::normalize_codec("av01"), "av1");
        assert_eq!(EncoderCatalog::normalize_codec("VP09"), "vp9");
        assert_eq!(EncoderCatalog::normalize_codec("weird"), "weird");
        assert_eq!(EncoderCatalog::normalize_codec("  "), "");
    }

    #[test]
    fn software_encoder_for_each_codec() {
        assert_eq!(EncoderCatalog::software_encoder_for("h264").map(|p| p.name.as_str()), Some("libx264"));
        assert_eq!(EncoderCatalog::software_encoder_for("hevc").map(|p| p.name.as_str()), Some("libx265"));
        // av1 有两个软件画像，取 sort_preference 最小者（libsvtav1 = 90）
        assert_eq!(EncoderCatalog::software_encoder_for("av1").map(|p| p.name.as_str()), Some("libsvtav1"));
        assert!(EncoderCatalog::software_encoder_for("prores").is_none());
    }

    #[test]
    fn encoder_type_round_trip() {
        for t in EncoderType::ALL {
            assert_eq!(EncoderType::from_id(t.id()), Some(*t));
            assert_eq!(EncoderType::from_int(t.to_int()), *t);
            if *t != EncoderType::Other {
                assert!(t.family_matches(t.id()));
            }
        }
        assert!(!EncoderType::Other.family_matches("other"));
    }
}
