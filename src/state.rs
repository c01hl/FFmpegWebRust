//! 应用共享状态（axum `State`）。
//!
//! 所有服务都是单例、内部持有 `Arc`、`Clone` 廉价 —— 与原实现把服务注册成 ASP.NET
//! 单例是同一个理由：**整个进程共享一份状态，页面（HTTP 请求）之间不互相影响**。
//!
//! 装配顺序也是有意的：先开数据库 → 设置 → FFmpeg 定位/能力 → 硬件检测 → 模板 → 队列。
//! 后者依赖前者的实例。

use std::sync::Arc;

use crate::cli::ListenConfig;
use crate::ffmpeg::hardware::HardwareDetector;
use crate::ffmpeg::service::FFmpegService;
use crate::queue::task_service::TaskService;
use crate::queue::ConversionQueue;
use crate::services::SettingsService;
use crate::store::Store;
use crate::templates::TemplateService;

/// 进程级共享状态
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub settings: Arc<SettingsService>,
    pub ffmpeg: Arc<FFmpegService>,
    pub hardware: Arc<HardwareDetector>,
    pub templates: Arc<TemplateService>,
    pub queue: Arc<ConversionQueue>,
    pub tasks: Arc<TaskService>,
    /// 实际生效的监听配置（`/api/settings/port` 用它报告「重启才生效」）
    pub listen: Arc<ListenConfig>,
}

impl AppState {
    /// 装配全部服务并完成启动初始化（对应 `Program.cs` 的 `InitializeApplicationAsync`）。
    ///
    /// 初始化失败**不应**阻止应用启动：否则用户连设置界面都打不开，
    /// 也就没法去把 FFmpeg 路径改对。
    pub async fn bootstrap(store: Store, listen: ListenConfig) -> anyhow::Result<Self> {
        let store = Arc::new(store);
        let settings = Arc::new(SettingsService::new(store.clone()));

        // 设置里可能指定了 ffmpeg/ffprobe 路径
        let current = settings.cached().await;
        let ffmpeg = Arc::new(FFmpegService::from_settings(&current));

        let hardware = Arc::new(HardwareDetector::with_cache_store(ffmpeg.clone(), store.clone()));
        hardware.apply_settings(&current);

        let templates = Arc::new(TemplateService::new(
            store.clone(),
            ffmpeg.clone(),
            hardware.clone(),
        ));

        let queue = Arc::new(ConversionQueue::new(
            store.clone(),
            settings.clone(),
            templates.clone(),
            ffmpeg.clone(),
            hardware.clone(),
        ));
        queue.start();

        let tasks = Arc::new(TaskService::new(
            store.clone(),
            queue.clone(),
            settings.clone(),
            templates.clone(),
            ffmpeg.clone(),
            hardware.clone(),
        ));

        let state = Self {
            store,
            settings,
            ffmpeg,
            hardware,
            templates,
            queue,
            tasks,
            listen: Arc::new(listen),
        };

        state.initialize().await;
        Ok(state)
    }

    /// 启动期初始化（失败只记日志，不阻止启动）
    async fn initialize(&self) {
        // 1) 上次非正常退出留下的「运行中」任务需要复位
        match self.tasks.recover_orphaned_tasks().await {
            Ok(count) if count > 0 => tracing::info!("已复位 {count} 个中断的任务"),
            Ok(_) => {}
            Err(error) => tracing::warn!("复位中断任务失败：{error}"),
        }

        // 2) 系统模板随应用版本同步
        match self.templates.initialize_system_templates().await {
            Ok(result) => tracing::info!("{}", result.summary),
            Err(error) => tracing::warn!("系统模板同步失败：{error}"),
        }

        // 3) 按设置清理过期历史
        let settings = self.settings.cached().await;
        if settings.task_history_retention_days > 0 {
            match self.tasks.cleanup_history(settings.task_history_retention_days).await {
                Ok((tasks, batches)) if tasks > 0 || batches > 0 => {
                    tracing::info!("已清理 {tasks} 条过期任务记录、{batches} 个批次");
                }
                Ok(_) => {}
                Err(error) => tracing::warn!("清理历史失败：{error}"),
            }
        }

        // 4) 设置变更时把新路径与硬件偏好同步给 FFmpeg 服务（设置页保存后立即生效）
        let mut receiver = self.settings.subscribe();
        let ffmpeg = self.ffmpeg.clone();
        let hardware = self.hardware.clone();
        tokio::spawn(async move {
            while let Ok(settings) = receiver.recv().await {
                // 重新定位 ffmpeg/ffprobe 并清空能力缓存，再同步硬件偏好
                ffmpeg.apply_settings(&settings);
                hardware.apply_settings(&settings);
            }
        });
    }

    /// FFmpeg 能力探测 + 硬件检测（后台执行，不阻塞启动）。
    ///
    /// // PORT NOTE（有意差异 #22）：硬件检测走 `force = true`。参考实现把检测结果
    /// 缓存 24 小时，但**缓存里不含滤镜清单**（见 `ffmpeg-core.md` §8.7），复用缓存会让
    /// `has_filter()` 恒为 true —— 于是「需要 libass 的烧录字幕模板」在重启后会被误判为
    /// 可用，直到真正跑转换才失败。每次启动完整检测一次的代价很小（本机约 0.2–2 秒），
    /// 换来的是「界面说能用就真能用」。
    pub fn spawn_capability_probe(&self) {
        let ffmpeg = self.ffmpeg.clone();
        let hardware = self.hardware.clone();

        tokio::spawn(async move {
            match ffmpeg.info(true).await {
                Some(info) if info.available => {
                    tracing::info!("FFmpeg {}（{}）", info.version, info.path);
                    if info.major_version > 0 && info.major_version < 6 {
                        tracing::warn!(
                            "当前 FFmpeg 主版本为 {}，较旧；建议升级到 7.x 以上以获得完整的硬件加速支持。",
                            info.major_version
                        );
                    }
                }
                _ => {
                    tracing::warn!("未能找到可用的 FFmpeg，请打开「设置」指定 FFmpeg 路径。");
                    return;
                }
            }

            // 完整检测（force = true）：见 spawn_forced_hardware_detection 的说明
            let report = hardware.detect(true).await;
            tracing::info!("{}", report.summary);
        });
    }

}
