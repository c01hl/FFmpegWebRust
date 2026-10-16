//! 服务层（对应参考实现 `Services/` 目录）。
//!
//! 目前只有设置服务；队列、模板服务、硬件检测等由其它模块提供。

pub mod settings;

pub use settings::SettingsService;
