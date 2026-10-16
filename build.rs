//! 构建脚本。
//!
//! 只做一件事：保证 `web/dist` 目录存在。
//!
//! `src/web_assets.rs` 用 `rust-embed` 在**编译期**把前端产物内嵌进二进制，
//! 而 `rust-embed` 要求被内嵌的目录必须存在 —— 直接在空仓库里 `cargo build`
//! 会因为目录不存在而编译失败。这里先把目录建出来（空目录也能编译，只是内嵌 0 个文件，
//! 服务端会给出「前端还没构建」的提示页）。
//!
//! 前端构建产物本身**不进版本库**：`cd web && pnpm install && pnpm build` 生成。

use std::path::Path;

fn main() {
    let dist = Path::new("web/dist");
    if !dist.exists() {
        let _ = std::fs::create_dir_all(dist);
    }

    // 前端产物变了要重新编译（否则改了界面看不到效果，很难排查）
    println!("cargo:rerun-if-changed=web/dist");

    // 前端源码变了让 cargo 知道（真正的内容变化由 dist 触发）
    println!("cargo:rerun-if-changed=web/index.html");
    println!("cargo:rerun-if-changed=web/package.json");
}
