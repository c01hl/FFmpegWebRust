//! 文件相关服务：输出路径解析（冲突策略）、文件名占位符、目录扫描、原生文件对话框。
//!
//! 对应参考实现的 `Services/OutputPathResolver.cs`、`Services/FileService.cs`、
//! `Services/FileDialogService.cs`。行为规格见 `docs/reference/backend-data.md` §4 / §7。

pub mod dialog;
pub mod file_service;
pub mod output_path;

pub use dialog::{DialogBackend, DialogKind, DialogRequest, DialogResult, backend as dialog_backend, pick as pick_path};
pub use file_service::{
    MediaFileEntry, available_disk_space, format_file_size, get_file_size, is_directory_writable,
    is_file_accessible, reveal_in_file_manager, scan_media_files, supported_media_extensions,
};
pub use output_path::{
    OutputPathResolution, format_file_name, generate_output_path, generate_unique_output_path,
    is_output_path_writable, resolve_output_path, sanitize_file_name,
};
