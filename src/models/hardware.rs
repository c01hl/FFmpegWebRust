//! 硬件编码器检测结果（持久化实体，对应参考实现 `Models/HardwareEncoder.cs`）。
//!
//! 注意：运行时检测报告用的是 `crate::ffmpeg::hardware::HardwareReport`（不落库）；
//! 本结构是它的**可持久化投影**，用于「设置页/首页」在检测尚未跑完时也能展示上次结果。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ffmpeg::encoder_catalog::EncoderType;

/// 一条编码器检测结果
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareEncoder {
    pub id: String,
    /// 编码器名，如 `h264_nvenc`（唯一）
    pub name: String,
    /// 展示名，如 `NVIDIA NVENC H.264`
    pub display_name: String,
    pub encoder_type: EncoderType,
    /// 厂商标识：`nvenc/qsv/amf/videotoolbox/vaapi/vulkan/mf/software`
    pub family: String,
    /// 目标编码格式：`h264/hevc/av1/vp9/…`
    pub codec: String,
    /// 是否可用（试跑通过）
    pub is_available: bool,
    /// 是否通过了带质量参数的完整试跑；false = 只能用最简参数
    pub supports_quality_options: bool,
    pub last_checked_at: DateTime<Utc>,
    /// 不可用原因
    pub unavailable_reason: Option<String>,
    /// ffmpeg 构建里根本没这个编码器
    pub missing_from_build: bool,
    /// 检测时使用的 ffmpeg 版本（缓存失效判断）
    pub ffmpeg_version: String,
}

impl HardwareEncoder {
    /// 是否硬件编码器（计算属性）
    pub fn is_hardware(&self) -> bool {
        self.encoder_type != EncoderType::Software
    }
}

impl Default for HardwareEncoder {
    fn default() -> Self {
        Self {
            id: crate::models::new_id(),
            name: String::new(),
            display_name: String::new(),
            encoder_type: EncoderType::Software,
            family: String::new(),
            codec: String::new(),
            is_available: false,
            supports_quality_options: true,
            last_checked_at: Utc::now(),
            unavailable_reason: None,
            missing_from_build: false,
            ffmpeg_version: String::new(),
        }
    }
}
