//! 二进制入口：解析启动参数 → 打开数据库 → 装配服务 → 启动 HTTP 服务。
//!
//! 与原实现的差异：参考实现需要在 Web 主机启动**之前**用一条旁路直接读 LiteDB 才能拿到
//! `ListenPort`（ASP.NET 的 `UseUrls` 必须在 DI 容器建立前确定）。这里 `main` 本身就是异步的，
//! 顺序执行「读设置 → 决定监听地址 → 启动服务」即可，不需要那套旁路读取。

use std::sync::Arc;

use anyhow::Context;
use ffmpeg_web_rust::cli::{self, CliOptions};
use ffmpeg_web_rust::paths;
use ffmpeg_web_rust::state::AppState;
use ffmpeg_web_rust::store::Store;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let options = CliOptions::parse(std::env::args().skip(1)).merge_env();

    // `--data-dir` 必须在任何 paths::* 调用之前落地（它与环境变量走同一条路径）
    if let Some(dir) = options.data_dir.clone() {
        // SAFETY: 单线程启动阶段，尚未创建读取环境变量的线程
        unsafe { std::env::set_var(paths::ENV_DATA_DIR, dir) };
    }

    let database_path = paths::database_path();
    tracing::info!("数据目录：{}", paths::data_directory().display());

    let store = Store::open(&database_path)
        .await
        .with_context(|| format!("打开数据库失败：{}", database_path.display()))?;

    let saved = store.load_settings().await.ok().flatten();
    // 启动参数 > 环境变量 > 数据库里保存的设置 > 默认端口
    let listen = cli::resolve_listen(&options, saved.as_ref().and_then(|s| u16::try_from(s.listen_port).ok()));
    let auto_open_browser =
        cli::should_auto_open_browser(&options, saved.as_ref().map(|s| s.auto_open_browser));

    let state = AppState::bootstrap(store, listen.clone()).await?;
    state.spawn_capability_probe();

    let app = ffmpeg_web_rust::api::router(state.clone());

    let listener = tokio::net::TcpListener::bind(listen.socket_addr())
        .await
        .with_context(|| format!("无法监听 {}（端口可能已被占用）", listen.socket_addr()))?;
    let bound = listener.local_addr()?;
    // 浏览器永远走回环地址（即使监听在 0.0.0.0，本机也应该用 localhost 打开）
    let browser_url = format!("http://localhost:{}", bound.port());

    if listen.loopback_only {
        tracing::info!("服务已启动：{browser_url}（仅本机可访问）");
    } else {
        // 监听在 0.0.0.0 / 具体 IP 时，把真实的可访问地址打出来，避免用户以为还是只能本机访问
        let display_host = if listen.bind == "0.0.0.0" || listen.bind == "[::]" || listen.bind == "::" {
            "<本机局域网 IP>".to_string()
        } else {
            listen.bind.clone()
        };
        tracing::info!("服务已启动：http://{display_host}:{}（监听 {}，可从局域网访问）", bound.port(), listen.bind);
        tracing::warn!(
            "注意：本程序能在本机执行 FFmpeg 并读写任意文件，且没有登录校验。\
             暴露到局域网前请确认这是你想要的效果，或在前面加一层带认证的反向代理。"
        );
    }

    if !ffmpeg_web_rust::web_assets::frontend_available() {
        tracing::warn!("内嵌的前端产物为空。请先执行 `cd web && pnpm install && pnpm build`，然后重新 `cargo build`。");
    }

    if auto_open_browser {
        // 稍等片刻再打开，避免浏览器先于监听就绪
        let target = browser_url;
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            cli::open_browser(&target);
        });
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(state))
        .await
        .context("HTTP 服务异常退出")?;

    tracing::info!("已退出");
    Ok(())
}

/// 启动日志：默认 info 级别，可用 `RUST_LOG` 覆盖
fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_target(false).try_init();
}

/// Ctrl-C / SIGTERM：先把队列里在跑的任务收尾，再退出
async fn shutdown_signal(state: AppState) {
    use tokio::signal;

    let ctrl_c = async {
        let _ = signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut stream) = signal::unix::signal(signal::unix::SignalKind::terminate()) {
            stream.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("收到中断信号，正在停止…"),
        _ = terminate => tracing::info!("收到终止信号，正在停止…"),
    }

    // 等正在执行的任务收尾（shutdown 内部有 10 秒超时，这里再兜一层）
    let queue = Arc::clone(&state.queue);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(12), queue.shutdown()).await;
}
