//! FFmpeg WebUI (Rust) —— 库入口。
//!
//! 模块划分与参考实现（C#/.NET Blazor）的对应关系见 `docs/architecture.md`。
//! 二进制入口 `src/main.rs` 只负责启动；所有能力都在本库中，便于 `tests/` 与
//! `src/bin/*` 下的自检工具直接调用。

pub mod cli;
pub mod command;
pub mod ffmpeg;
pub mod files;
pub mod models;
pub mod paths;
pub mod services;
pub mod store;
pub mod templates;
pub mod web_assets;
