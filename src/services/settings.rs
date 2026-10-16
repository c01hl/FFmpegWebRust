//! 全局设置服务（对应参考实现 `Services/SettingsService.cs`，行为见
//! `docs/reference/backend-data.md` §5）。
//!
//! 职责：
//! - 进程内**单份缓存**：命中缓存不访问数据库（队列/API 高频读取）；
//! - 首次运行落一份默认值（`default_output_directory` 取系统「视频」目录）；
//! - 保存/重置后刷新缓存并 **广播** 给订阅者（清 ffmpeg 路径缓存、清探测结果缓存）；
//! - ffmpeg 路径校验与版本读取（子进程，绝不阻塞 tokio 运行时）。

use std::sync::Arc;
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::{RwLock, broadcast};

use crate::models::AppSettings;
use crate::store::Store;

/// 探测 ffmpeg 版本的超时（参考实现是 10 秒，这里收紧到 5 秒：本地工具应当秒回，
/// 挂住的路径更像写错了）
const FFMPEG_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// 设置变更广播通道容量（有界，订阅者来不及消费时老消息会被丢弃——设置读取有缓存兜底）
const SETTINGS_BROADCAST_CAPACITY: usize = 32;

/// 全局设置服务（`Clone` 廉价：内部共享同一份缓存与广播通道）
#[derive(Clone)]
pub struct SettingsService {
    /// 数据库句柄
    store: Arc<Store>,
    /// 内存快照；`None` = 尚未从数据库读过
    snapshot: Arc<RwLock<Option<AppSettings>>>,
    /// 设置变更广播（保存/重置后触发）
    sender: broadcast::Sender<AppSettings>,
}

impl SettingsService {
    /// 创建服务（不同步读库；首次 `get()` 时才落默认值）
    pub fn new(store: Arc<Store>) -> Self {
        let (sender, _receiver) = broadcast::channel(SETTINGS_BROADCAST_CAPACITY);
        Self {
            store,
            snapshot: Arc::new(RwLock::new(None)),
            sender,
        }
    }

    /// 读取设置；首次运行会落一份默认值（default_output_directory 取自 paths::default_output_directory）
    ///
    /// 返回值一定已经过 `AppSettings::normalize()`。返回的是**快照副本**，
    /// 调用方就地修改不会污染全局缓存（参考实现返回共享引用，见 §5.2 的隐患标注）。
    pub async fn get(&self) -> AppSettings {
        if let Some(settings) = self.snapshot.read().await.clone() {
            return settings;
        }

        match self.store.load_settings().await {
            Ok(Some(mut settings)) => {
                settings.normalize();
                self.set_snapshot(settings.clone()).await;
                settings
            }
            Ok(None) => {
                // 首次运行：落一份默认值；写失败只在内存里退让（与参考实现一致，不影响界面可用）
                let settings = default_settings();
                if let Err(error) = self.store.save_settings(&settings).await {
                    tracing::warn!("写入默认设置失败，本次仅在内存中使用默认值：{error:#}");
                }
                self.set_snapshot(settings.clone()).await;
                settings
            }
            Err(error) => {
                // 读库失败 ≠ 没有记录：不写缓存也不落库，下次调用再试
                tracing::warn!("读取设置失败，本次使用默认值：{error:#}");
                default_settings()
            }
        }
    }

    /// 保存设置：规范化 → 落库 → 刷新缓存 → 广播；返回规范化后的值
    ///
    /// PORT NOTE：参考实现在这一步**吞掉**落库异常（内存状态照旧更新，数据库可能没写进去）。
    /// 这里把错误如实返回给调用方（设置页才能提示「保存失败」），
    /// 同时仍然刷新内存缓存 —— 运行期行为（并发数、日志行数等）与用户刚点的值保持一致。
    pub async fn save(&self, mut settings: AppSettings) -> anyhow::Result<AppSettings> {
        settings.normalize();

        // 沿用已有记录的 Id：settings 是单条记录，换 Id 会让「第一条」语义错位
        if let Some(existing_id) = self.existing_settings_id().await {
            settings.id = existing_id;
        }

        let result = self.store.save_settings(&settings).await;
        self.set_snapshot(settings.clone()).await;
        self.broadcast(settings.clone());
        result?;
        Ok(settings)
    }

    /// 恢复默认设置：写入默认值（含默认输出目录）并广播
    pub async fn reset(&self) -> anyhow::Result<AppSettings> {
        let settings = default_settings();
        let result = self.store.save_settings(&settings).await;
        self.set_snapshot(settings.clone()).await;
        self.broadcast(settings.clone());
        result?;
        Ok(settings)
    }

    /// 订阅设置变更（保存/重置后收到一份新的规范化设置）
    pub fn subscribe(&self) -> broadcast::Receiver<AppSettings> {
        self.sender.subscribe()
    }

    /// 只读缓存（不访问数据库），队列与 API 高频调用
    ///
    /// 缓存尚未建立时返回默认设置快照（**不会**写库、也不会污染缓存），
    /// 因此调用方首次仍应走一次 `get()`。
    pub async fn cached(&self) -> AppSettings {
        match self.snapshot.read().await.clone() {
            Some(settings) => settings,
            None => default_settings(),
        }
    }

    /// 校验 ffmpeg 可执行文件：执行 `<path> -version`（5 秒超时），
    /// 输出里要有以 `ffmpeg version` 开头的行。
    ///
    /// 返回 `(是否合法, 版本行或失败原因)`。路径不存在 → `(false, "文件不存在：{path}")`。
    pub async fn validate_ffmpeg_path(&self, path: &str) -> (bool, String) {
        let exe = match resolve_executable(path) {
            Ok(exe) => exe,
            Err(reason) => return (false, reason),
        };

        match run_version(&exe).await {
            Ok(output) => match extract_version_line(&output) {
                Some(line) => (true, line),
                None => (
                    false,
                    format!("不是有效的 ffmpeg：{}", first_line(&output)),
                ),
            },
            Err(error) => (false, format!("{error:#}")),
        }
    }

    /// 读取 ffmpeg 版本行（`ffmpeg version …`，已截断到 `Copyright` 之前）；失败返回 `None`
    pub async fn ffmpeg_version(&self, path: &str) -> Option<String> {
        let exe = resolve_executable(path).ok()?;
        let output = run_version(&exe).await.ok()?;
        extract_version_line(&output)
    }

    /// 写入内存快照
    async fn set_snapshot(&self, settings: AppSettings) {
        *self.snapshot.write().await = Some(settings);
    }

    /// 广播（没有订阅者时 `send` 会失败，属正常情况，忽略）
    fn broadcast(&self, settings: AppSettings) {
        let _ = self.sender.send(settings);
    }

    /// 已有设置记录的 Id（缓存优先，其次读库）
    async fn existing_settings_id(&self) -> Option<String> {
        if let Some(settings) = self.snapshot.read().await.as_ref() {
            return Some(settings.id.clone());
        }
        match self.store.load_settings().await {
            Ok(Some(settings)) => Some(settings.id),
            _ => None,
        }
    }
}

/// 默认设置（`default_output_directory` 取系统「视频」目录，见 `paths::default_output_directory`）
fn default_settings() -> AppSettings {
    let mut settings = AppSettings::default();
    settings.default_output_directory = crate::paths::default_output_directory()
        .to_string_lossy()
        .to_string();
    settings.normalize();
    settings
}

/// 把用户填的路径解析成可执行的命令名。
///
/// - 空串 = 自动探测：直接用字面量 `ffmpeg`，交给操作系统按 `PATH` 查找；
/// - 非空：先词法规整成绝对路径（`paths::to_absolute`，与打开文件对话框的起始目录一致），
///   文件不存在直接返回中文原因。
fn resolve_executable(path: &str) -> Result<String, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Ok("ffmpeg".to_string());
    }

    let absolute = crate::paths::to_absolute(trimmed);
    if !absolute.exists() {
        return Err(format!("文件不存在：{trimmed}"));
    }
    Ok(absolute.to_string_lossy().to_string())
}

/// 执行 `{exe} -version`（5 秒超时），返回文本输出；stdout 为空时回退到 stderr
///
/// 用 `tokio::process::Command`，并且 `kill_on_drop(true)`：
/// 超时后 future 被丢弃时会连带杀掉子进程，避免留下挂死的 ffmpeg。
async fn run_version(exe: &str) -> anyhow::Result<String> {
    let mut command = Command::new(exe);
    command
        .arg("-version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let child = command
        .spawn()
        .map_err(|error| anyhow::anyhow!("无法执行「{exe}」：{error}"))?;

    let output = match tokio::time::timeout(FFMPEG_PROBE_TIMEOUT, child.wait_with_output()).await {
        Ok(result) => result.map_err(|error| anyhow::anyhow!("等待「{exe}」退出失败：{error}"))?,
        Err(_) => {
            return Err(anyhow::anyhow!(
                "执行「{exe} -version」超时（{} 秒）",
                FFMPEG_PROBE_TIMEOUT.as_secs()
            ));
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if stdout.trim().is_empty() {
        // 有些构建把版本信息写到 stderr（原实现同样做了这个回退）
        return Ok(String::from_utf8_lossy(&output.stderr).to_string());
    }
    Ok(stdout)
}

/// 取第一条以 `ffmpeg version` 开头的行（忽略大小写），并截断到 `Copyright` 之前
fn extract_version_line(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.to_ascii_lowercase().starts_with("ffmpeg version") {
            let truncated = match line.find("Copyright") {
                Some(index) => line[..index].trim(),
                None => line,
            };
            return Some(truncated.to_string());
        }
    }
    None
}

/// 输出里的第一条非空行（用于失败时的中文提示，最长 200 字符）
fn first_line(text: &str) -> String {
    for line in text.lines() {
        let line = line.trim();
        if !line.is_empty() {
            let mut line = line.to_string();
            if line.chars().count() > 200 {
                line = line.chars().take(200).collect::<String>() + "…";
            }
            return line;
        }
    }
    "（无输出）".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_is_extracted_and_truncated() {
        let output = "\nffmpeg version 7.1 Copyright (c) 2000-2024 the FFmpeg developers\nbuilt with clang\n";
        assert_eq!(
            extract_version_line(output).as_deref(),
            Some("ffmpeg version 7.1")
        );
    }

    #[test]
    fn version_line_requires_prefix() {
        assert!(extract_version_line("FFMPEG VERSION 7.0").is_some()); // 忽略大小写
        assert!(extract_version_line("not ffmpeg version 1").is_none());
        assert!(extract_version_line("").is_none());
    }

    #[test]
    fn empty_path_falls_back_to_path_lookup() {
        assert_eq!(resolve_executable("  ").as_deref(), Ok("ffmpeg"));
    }

    #[test]
    fn missing_path_reports_chinese_reason() {
        let reason = resolve_executable("/definitely/not/here/ffmpeg");
        assert!(reason.is_err());
        assert!(reason.unwrap_err().starts_with("文件不存在："));
    }

    #[test]
    fn default_settings_have_output_directory() {
        let settings = default_settings();
        assert!(!settings.default_output_directory.is_empty());
        assert_eq!(settings.max_concurrent_tasks, 1);
        assert_eq!(settings.theme, "System");
    }
}
