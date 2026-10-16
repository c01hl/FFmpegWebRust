//! 领域模型（与参考实现 `Models/` 目录一一对应）。
//!
//! `EncoderType` 只有一份定义，放在 `crate::ffmpeg::encoder_catalog` 里：
//! 编码器画像表与硬件检测缓存必须使用同一个类型，否则两者无法互相比较。

pub mod dto;
pub mod enums;
pub mod hardware;
pub mod settings;
pub mod task;
pub mod template;

pub use dto::{TemplateIssue, TemplateTestResult};
pub use enums::{
    FileExistsAction, HardwareAccelerationMode, OutputNamingRule, TaskStatus, TemplateIssueSeverity,
    TemplateParameterKind, TemplateType,
};
pub use hardware::HardwareEncoder;
pub use settings::AppSettings;
pub use task::{BatchTask, ConversionTask};
pub use template::{CommandTemplate, TemplateParameter, TemplateParameterOption};

/// 与参考实现共用的编码器类型
pub use crate::ffmpeg::encoder_catalog::EncoderType;

use rand::RngExt;
use std::sync::atomic::{AtomicU32, Ordering};

/// 递增计数器，保证同一秒内生成的 id 不重复（与原实现 ObjectId 的 3 字节计数器一致）
static OID_COUNTER: AtomicU32 = AtomicU32::new(0);

/// 生成 24 位小写十六进制 id，格式与参考实现的 `ObjectId` 完全一致：
/// 4 字节大端秒级时间戳 + 5 字节随机 + 3 字节递增计数器。
///
/// 前端路由、`batchId`、`templateId` 都直接使用这个字符串，因此格式必须保持 24 位十六进制。
pub fn new_id() -> String {
    let seconds = chrono::Utc::now().timestamp() as u32;
    let mut bytes = [0u8; 12];
    bytes[0..4].copy_from_slice(&seconds.to_be_bytes());

    let mut rng = rand::rng();
    let random: [u8; 5] = rng.random();
    bytes[4..9].copy_from_slice(&random);

    let counter = OID_COUNTER.fetch_add(1, Ordering::Relaxed) & 0x00FF_FFFF;
    bytes[9] = (counter >> 16) as u8;
    bytes[10] = (counter >> 8) as u8;
    bytes[11] = counter as u8;

    let mut out = String::with_capacity(24);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// 判断字符串是否是一个合法的 24 位十六进制 id
pub fn is_valid_id(value: &str) -> bool {
    value.len() == 24 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_objectid_shaped_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..10_000 {
            let id = new_id();
            assert_eq!(id.len(), 24, "长度必须是 24：{id}");
            assert!(is_valid_id(&id), "必须是十六进制：{id}");
            assert!(seen.insert(id), "id 重复");
        }
    }

    #[test]
    fn id_prefix_encodes_timestamp() {
        let id = new_id();
        let seconds = u32::from_str_radix(&id[0..8], 16).expect("时间戳段必须是十六进制");
        let now = chrono::Utc::now().timestamp() as u32;
        assert!(now.abs_diff(seconds) < 5, "时间戳段应接近当前时间");
    }
}
