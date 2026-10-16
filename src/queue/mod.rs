//! 后台转换队列（对应参考实现 `Services/ConversionQueue.cs`）。
//!
//! 行为规格：`docs/reference/backend-data.md` §3（执行引擎生命周期）+ §4.1–4.4（输出路径解析）
//! + §2.3；`docs/reference/templates.md` §3.7（`CommandContext` 装配）与 §5.2（编码器选择）。
//!
//! # 不变式
//!
//! 1. **与 UI 连接无关**：worker 是常驻 tokio 任务，页面刷新 / WebSocket 断线不影响正在跑的转换
//!    （参考实现最重要的不变式）。
//! 2. **同一任务只执行一次**：`enqueue` 按 `task_id` 幂等（本项目有意差异 #5）。
//! 3. **绝不覆盖源文件**：输出路径与输入相同时强制改名（§4.1 第 2 条，优先于一切策略）。
//! 4. **不让 ffmpeg 读 stdin**：命令行永远带 `-y` 或 `-n`（`RunRequest::overwrite`）。
//!
//! # 本文件的分工
//!
//! - `engine.rs`：队列本体（入队、并发、单任务 18 步生命周期、取消、停机、事件）；
//! - `plan.rs`：执行期的纯计算（目标 codec 推断、音频编码器、硬件解码参数、回退判定、日志截断）；
//! - `task_service.rs` / `preview.rs`：由另一位实现者负责（任务 CRUD、重试、预览、孤儿恢复）。

pub mod engine;
pub mod plan;
pub mod preview;
pub mod task_service;

pub use engine::{ConversionQueue, QueueEvent};
