//! 统一错误响应：`{"error":{"code","message"}}`（见 `docs/api-contract.md` §0）。
//!
//! 约定：**message 直接展示给用户**，因此必须是中文、可读、不含内部细节
//! （堆栈、SQL、路径之外的实现信息都不要放进来）。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// 接口错误
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    /// 供前端分支判断的稳定代码，如 `TEMPLATE_NOT_FOUND`
    pub code: &'static str,
    /// 面向用户的中文说明
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into() }
    }

    pub fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    pub fn not_found(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, code, message)
    }

    pub fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }

    pub fn forbidden(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, code, message)
    }

    /// 内部错误：详情只进日志，不返回给前端
    pub fn internal(context: &str, error: impl std::fmt::Display) -> Self {
        tracing::error!("{context}：{error}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            "服务内部错误，请查看服务端日志。",
        )
    }

    pub fn from_anyhow(context: &str, error: anyhow::Error) -> Self {
        Self::internal(context, error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "error": { "code": self.code, "message": self.message } })),
        )
            .into_response()
    }
}

/// 把 `anyhow::Result<T>` 转成接口结果（内部错误统一映射）
pub trait IntoApiResult<T> {
    fn api(self, context: &str) -> Result<T, ApiError>;
}

impl<T> IntoApiResult<T> for anyhow::Result<T> {
    fn api(self, context: &str) -> Result<T, ApiError> {
        self.map_err(|error| ApiError::from_anyhow(context, error))
    }
}
