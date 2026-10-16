//! 内嵌前端产物与静态资源托管。
//!
//! 前端（`web/`）由 Vite 构建到 `web/dist`，编译期被 `rust-embed` 打进二进制。
//! 因此发布物是**单个可执行文件**，目标机器不需要 Node。
//!
//! 路由策略（与原实现的 `MapStaticAssets` + SPA 回落等价）：
//! - 命中真实文件 → 返回文件（带 `Content-Type` 与长缓存头，文件名带内容指纹）
//! - 未命中且不是 `/api/*` → 返回 `index.html`（交给 React Router 处理）
//! - 未命中且路径看起来像静态资源（有扩展名）→ 404

use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

/// 前端构建产物。`web/dist` 不存在时（例如还没跑过 `pnpm build`）退化为空资源集，
/// 服务仍然能启动，只是页面会提示需要先构建前端。
#[derive(RustEmbed)]
#[folder = "web/dist"]
struct FrontendAssets;

/// 单个资源
pub struct Asset {
    pub content: Vec<u8>,
    pub content_type: String,
}

/// 查找资源；`path` 不带前导 `/`
fn find(path: &str) -> Option<Asset> {
    let path = path.trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    FrontendAssets::get(path).map(|file| Asset {
        content_type: mime_guess::from_path(path).first_or_octet_stream().to_string(),
        content: file.data.into_owned(),
    })
}

/// 内嵌资源里是否有 `index.html`（即前端是否已经构建过）
pub fn frontend_available() -> bool {
    FrontendAssets::get("index.html").is_some()
}

/// 前端构建产物的文件数（诊断用）
pub fn asset_count() -> usize {
    FrontendAssets::iter().count()
}

/// axum 的兜底处理器：静态资源 + SPA 回落
pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path();

    if let Some(asset) = find(path) {
        return build_response(asset, path);
    }

    // 静态资源没命中（有扩展名）→ 真的 404，不要回落成 HTML，
    // 否则浏览器会把 HTML 当 JS/CSS 解析，报出误导性的语法错误。
    if looks_like_asset(path) {
        return (StatusCode::NOT_FOUND, "404 Not Found").into_response();
    }

    match find("index.html") {
        Some(asset) => build_response(asset, "index.html"),
        None => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            FRONTEND_NOT_BUILT_HTML,
        )
            .into_response(),
    }
}

fn looks_like_asset(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .map(|name| name.contains('.'))
        .unwrap_or(false)
}

fn build_response(asset: Asset, path: &str) -> Response {
    // Vite 产物文件名带内容 hash，可以长缓存；index.html 必须每次校验。
    let cache_control = if path == "index.html" || path.is_empty() {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, asset.content_type)
        .header(header::CACHE_CONTROL, cache_control)
        .body(Body::from(asset.content))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// 前端还没构建时的提示页（开发期最容易踩的坑，直接把命令写在页面上）
const FRONTEND_NOT_BUILT_HTML: &str = r#"<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8"><title>FFmpeg WebUI (Rust)</title>
<style>
 body{font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;background:#0f172a;color:#e2e8f0;
      display:flex;min-height:100vh;align-items:center;justify-content:center;margin:0}
 .card{max-width:640px;background:#1e293b;border:1px solid #334155;border-radius:12px;padding:32px}
 h1{font-size:20px;margin:0 0 12px}code{background:#0f172a;padding:2px 6px;border-radius:4px;color:#7dd3fc}
 p{line-height:1.7;color:#cbd5e1}pre{background:#0f172a;padding:12px;border-radius:8px;overflow:auto}
</style></head><body><div class="card">
<h1>前端还没有构建</h1>
<p>服务端已经起来了，但没有找到内嵌的前端产物。请先构建前端：</p>
<pre><code>cd web &amp;&amp; pnpm install &amp;&amp; pnpm build</code></pre>
<p>然后再重新编译并运行本程序（前端产物是在 <strong>编译期</strong> 被内嵌进二进制的）：</p>
<pre><code>cargo run</code></pre>
<p>开发前端时也可以直接跑 <code>cd web &amp;&amp; pnpm dev</code>，Vite 会把 <code>/api</code> 代理到本服务。</p>
</div></body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_detection() {
        assert!(looks_like_asset("/assets/index-abc123.js"));
        assert!(looks_like_asset("/favicon.svg"));
        assert!(!looks_like_asset("/convert"));
        assert!(!looks_like_asset("/"));
    }

    #[test]
    fn empty_path_resolves_to_index() {
        // 未构建前端时返回 None，但不应 panic
        let _ = find("");
        let _ = find("/");
    }
}
