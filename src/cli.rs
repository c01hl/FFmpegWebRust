//! 启动参数解析：绑定地址、端口、是否自动打开浏览器。
//!
//! 优先级（与原实现一致，见 `docs/reference/backend-data.md` §9）：
//!
//! - 监听地址：`--bind` > `FFMPEGWEBRUST_BIND` > 默认 `127.0.0.1`（**仅本机**）
//! - 监听端口：`--port` > `FFMPEGWEBRUST_PORT` > 数据库里保存的设置 > 默认 5266
//! - 自动打开浏览器：`--no-browser` / `FFMPEGWEBRUST_NO_BROWSER=1` > 数据库设置 > 默认 true
//!
//! 与原实现的差异：原实现有一个「Web 主机启动前直接读一次数据库」的特殊路径
//! （ASP.NET 的 `UseUrls` 必须在 DI 容器建立前确定）。这里 `main` 本身就是异步的，
//! 可以先异步打开数据库、读出设置、再决定监听地址，因此不需要那套旁路读取。

use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};

use crate::paths::{self, ENV_BIND, ENV_NO_BROWSER, ENV_PORT};

/// 默认端口（与参考实现的 5265 错开，方便两个应用同时运行）
pub const DEFAULT_PORT: u16 = 5266;

/// 命令行选项
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CliOptions {
    pub port: Option<u16>,
    pub bind: Option<String>,
    pub no_browser: bool,
    pub data_dir: Option<String>,
}

impl CliOptions {
    /// 解析 `--port 8080` / `-p 8080` / `--bind 0.0.0.0` / `--no-browser` / `--data-dir /tmp/x`
    pub fn parse<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let args: Vec<String> = args.into_iter().map(|a| a.as_ref().to_string()).collect();
        let mut options = CliOptions::default();
        let mut index = 0;

        // 取下一个参数作为选项的值（没有下一个就返回 None）
        fn value_at(args: &[String], index: usize) -> Option<String> {
            args.get(index + 1).map(|v| v.trim().to_string())
        }

        while index < args.len() {
            match args[index].as_str() {
                "--port" | "-p" => {
                    if let Some(port) = value_at(&args, index).and_then(|v| v.parse::<u16>().ok()) {
                        options.port = Some(port);
                        index += 1;
                    }
                }
                "--bind" | "--host" => {
                    if let Some(bind) = value_at(&args, index).filter(|v| !v.is_empty()) {
                        options.bind = Some(bind);
                        index += 1;
                    }
                }
                "--data-dir" => {
                    if let Some(dir) = value_at(&args, index).filter(|v| !v.is_empty()) {
                        options.data_dir = Some(dir);
                        index += 1;
                    }
                }
                "--no-browser" => options.no_browser = true,
                _ => {}
            }

            index += 1;
        }

        options
    }

    /// 从环境变量补全（命令行优先级更高）
    pub fn merge_env(&self) -> Self {
        let mut merged = self.clone();
        if merged.port.is_none() {
            merged.port = paths::env_var(ENV_PORT).and_then(|v| v.parse::<u16>().ok());
        }
        if merged.bind.is_none() {
            merged.bind = paths::env_var(ENV_BIND);
        }
        if !merged.no_browser {
            merged.no_browser = paths::env_var(ENV_NO_BROWSER).as_deref() == Some("1");
        }
        if merged.data_dir.is_none() {
            merged.data_dir = paths::env_var(paths::ENV_DATA_DIR);
        }
        merged
    }
}

/// 最终监听配置
#[derive(Debug, Clone, PartialEq)]
pub struct ListenConfig {
    /// 监听地址
    pub bind: String,
    /// 实际端口（当 `port == 0` 时这里仍是 0，表示交给系统随机分配）
    pub port: u16,
    /// 是否只监听回环地址（默认行为）
    pub loopback_only: bool,
}

impl ListenConfig {
    /// `http://127.0.0.1:5266` 形式；端口为 0 时只输出 `http://host`（由系统决定端口）
    pub fn url(&self) -> String {
        if self.port == 0 {
            format!("http://{}", self.bind)
        } else {
            format!("http://{}:{}", self.bind, self.port)
        }
    }

    /// 用于 socket 绑定的地址
    pub fn socket_addr(&self) -> std::net::SocketAddr {
        let ip = self.bind.parse::<std::net::IpAddr>().unwrap_or(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST));
        std::net::SocketAddr::new(ip, self.port)
    }
}

/// 解析最终监听配置。`saved_port` 来自数据库里保存的设置。
///
/// 端口为 0 时表示「自动挑一个空闲端口」：从 `DEFAULT_PORT` 开始向上找 100 个，
/// 全被占用才交给系统随机分配（与原实现的 `FindFreePort` 一致）。
pub fn resolve_listen(cli: &CliOptions, saved_port: Option<u16>) -> ListenConfig {
    let bind = cli
        .bind
        .clone()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let loopback_only = bind == "127.0.0.1" || bind == "localhost" || bind == "::1";

    let requested = match cli.port.or(saved_port) {
        Some(port) => port,
        None => DEFAULT_PORT,
    };

    let port = if requested == 0 {
        find_free_port(DEFAULT_PORT).unwrap_or(0)
    } else {
        requested
    };

    ListenConfig { bind, port, loopback_only }
}

/// 是否自动打开浏览器：命令行/环境变量优先，其次数据库设置，最后默认开启
pub fn should_auto_open_browser(cli: &CliOptions, saved: Option<bool>) -> bool {
    if cli.no_browser {
        return false;
    }
    saved.unwrap_or(true)
}

/// 从 `preferred` 起找一个空闲端口（最多试 100 个）
pub fn find_free_port(preferred: u16) -> Option<u16> {
    (preferred..preferred.saturating_add(100)).find(|port| is_port_free(*port))
}

/// 端口是否空闲（能绑上就认为空闲）
pub fn is_port_free(port: u16) -> bool {
    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).is_ok()
}

/// 用系统默认浏览器打开地址；无桌面环境时静默忽略
pub fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let command = ("open", vec![url.to_string()]);
    #[cfg(target_os = "windows")]
    let command = ("cmd", vec!["/c".to_string(), "start".to_string(), url.to_string()]);
    #[cfg(all(unix, not(target_os = "macos")))]
    let command = ("xdg-open", vec![url.to_string()]);

    let _ = std::process::Command::new(command.0)
        .args(command.1)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_port_and_bind() {
        let options = CliOptions::parse(["--port", "8080", "--bind", "0.0.0.0"]);
        assert_eq!(options.port, Some(8080));
        assert_eq!(options.bind.as_deref(), Some("0.0.0.0"));
    }

    #[test]
    fn parse_short_port_and_flags() {
        let options = CliOptions::parse(["-p", "9000", "--no-browser", "--data-dir", "/tmp/x"]);
        assert_eq!(options.port, Some(9000));
        assert!(options.no_browser);
        assert_eq!(options.data_dir.as_deref(), Some("/tmp/x"));
    }

    #[test]
    fn parse_ignores_unknown_and_missing_values() {
        let options = CliOptions::parse(["--port", "abc", "--wat", "--bind"]);
        assert_eq!(options.port, None);
        assert_eq!(options.bind, None);
    }

    #[test]
    fn resolve_prefers_cli_then_saved_then_default() {
        let cli = CliOptions { port: Some(1234), ..CliOptions::default() };
        assert_eq!(resolve_listen(&cli, Some(4321)).port, 1234);

        let cli = CliOptions::default();
        assert_eq!(resolve_listen(&cli, Some(4321)).port, 4321);
        assert_eq!(resolve_listen(&cli, None).port, DEFAULT_PORT);
    }

    #[test]
    fn resolve_binds_loopback_by_default() {
        let config = resolve_listen(&CliOptions::default(), Some(DEFAULT_PORT));
        assert!(config.loopback_only);
        assert_eq!(config.bind, "127.0.0.1");
        assert_eq!(config.url(), format!("http://127.0.0.1:{DEFAULT_PORT}"));
    }

    #[test]
    fn resolve_picks_free_port_when_zero() {
        let cli = CliOptions { port: Some(0), ..CliOptions::default() };
        let config = resolve_listen(&cli, None);
        assert_ne!(config.port, 0, "应该挑到一个空闲端口");
        assert!(is_port_free(config.port));
    }

    #[test]
    fn auto_open_browser_respects_no_browser_flag() {
        let cli = CliOptions { no_browser: true, ..CliOptions::default() };
        assert!(!should_auto_open_browser(&cli, Some(true)));
        assert!(should_auto_open_browser(&CliOptions::default(), None));
        assert!(!should_auto_open_browser(&CliOptions::default(), Some(false)));
    }
}
