//! 模板接口（`docs/api-contract.md` §1.3）。

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::api::error::{ApiError, IntoApiResult};
use crate::models::CommandTemplate;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_templates).post(create_template))
        .route("/categories", get(list_categories))
        .route("/validate", post(validate_template))
        .route("/export", get(export_templates))
        .route("/import", post(import_templates))
        .route("/system/reset", post(reset_system_templates))
        .route("/system/sync", post(sync_system_templates))
        .route("/{id}", get(get_template).put(update_template).delete(delete_template))
        .route("/{id}/copy", post(copy_template))
        .route("/{id}/test", post(test_template))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    #[serde(default = "default_true")]
    include_system: bool,
    #[serde(default)]
    category: Option<String>,
    /// 关键字：匹配名称、描述、标签
    #[serde(default)]
    q: Option<String>,
}

fn default_true() -> bool {
    true
}

/// `GET /api/templates`
async fn list_templates(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut items = state.templates.list(query.include_system).await.api("读取模板失败")?;

    if let Some(category) = query.category.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        items.retain(|item| item.template.category == category);
    }

    if let Some(keyword) = query.q.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        let keyword = keyword.to_lowercase();
        items.retain(|item| {
            let template = &item.template;
            template.name.to_lowercase().contains(&keyword)
                || template.description.to_lowercase().contains(&keyword)
                || template
                    .tags
                    .iter()
                    .any(|tag| tag.to_lowercase().contains(&keyword))
        });
    }

    // 契约：这个端点是**裸数组**（模板总量只有几十条，不需要分页；
    // 前端 `templatesApi.list()` 的类型就是 `TemplateListItem[]`）
    Ok(Json(json!(items)))
}

/// `GET /api/templates/categories`
async fn list_categories(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let categories = state.templates.categories().await.api("读取模板分类失败")?;
    Ok(Json(json!(categories)))
}

/// `GET /api/templates/{id}`
async fn get_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<CommandTemplate>, ApiError> {
    state
        .templates
        .get(&id)
        .await
        .api("读取模板失败")?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("TEMPLATE_NOT_FOUND", "模板不存在或已被删除。"))
}

/// `POST /api/templates`
async fn create_template(
    State(state): State<AppState>,
    Json(template): Json<CommandTemplate>,
) -> Result<(axum::http::StatusCode, Json<CommandTemplate>), ApiError> {
    let created = state
        .templates
        .create(template)
        .await
        .map_err(|error| template_write_error(&error.to_string()))?;

    Ok((axum::http::StatusCode::CREATED, Json(created)))
}

/// `PUT /api/templates/{id}`
async fn update_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(mut template): Json<CommandTemplate>,
) -> Result<Json<CommandTemplate>, ApiError> {
    template.id = id;
    let updated = state
        .templates
        .update(template)
        .await
        .map_err(|error| template_write_error(&error.to_string()))?;
    Ok(Json(updated))
}

/// `DELETE /api/templates/{id}`
async fn delete_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let deleted = state
        .templates
        .delete(&id)
        .await
        .map_err(|error| template_write_error(&error.to_string()))?;

    if !deleted {
        return Err(ApiError::not_found("TEMPLATE_NOT_FOUND", "模板不存在或已被删除。"));
    }
    Ok(Json(json!({ "deleted": true })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CopyRequest {
    #[serde(default)]
    new_name: Option<String>,
}

/// `POST /api/templates/{id}/copy`
async fn copy_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<CopyRequest>,
) -> Result<(axum::http::StatusCode, Json<CommandTemplate>), ApiError> {
    let copy = state
        .templates
        .copy(&id, request.new_name)
        .await
        .map_err(|error| template_write_error(&error.to_string()))?;
    Ok((axum::http::StatusCode::CREATED, Json(copy)))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TestRequest {
    #[serde(default)]
    parameter_values: HashMap<String, String>,
}

/// `POST /api/templates/{id}/test` —— 同步返回（首次会生成测试素材，最长约 30 秒）
async fn test_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<TestRequest>,
) -> Result<Json<crate::models::TemplateTestResult>, ApiError> {
    let template = state
        .templates
        .get(&id)
        .await
        .api("读取模板失败")?
        .ok_or_else(|| ApiError::not_found("TEMPLATE_NOT_FOUND", "模板不存在或已被删除。"))?;

    let result = state
        .templates
        .test(&template, &request.parameter_values)
        .await
        .api("模板试运行失败")?;
    Ok(Json(result))
}

/// `POST /api/templates/validate` —— 校验一个**未保存**的模板草稿
async fn validate_template(
    State(state): State<AppState>,
    Json(template): Json<CommandTemplate>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let filters = state.hardware.detect(false).await.supported_filters;
    let issues = state.templates.validate(&template, Some(&filters));
    Ok(Json(json!({ "issues": issues })))
}

/// `GET /api/templates/export?ids=a,b`
async fn export_templates(
    State(state): State<AppState>,
    Query(query): Query<ExportQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let all = state.templates.list(true).await.api("读取模板失败")?;
    let mut templates: Vec<CommandTemplate> = all.into_iter().map(|item| item.template).collect();

    if let Some(ids) = query.ids.as_deref().filter(|ids| !ids.trim().is_empty()) {
        let wanted: Vec<&str> = ids.split(',').map(str::trim).filter(|id| !id.is_empty()).collect();
        templates.retain(|template| wanted.contains(&template.id.as_str()));
    }

    let value = state.templates.export_json(&templates);
    let json_text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string());

    Ok(Json(json!({ "json": json_text, "fileName": "ffmpeg-templates.json" })))
}

#[derive(Debug, Deserialize)]
struct ExportQuery {
    #[serde(default)]
    ids: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportRequest {
    json: String,
    #[serde(default)]
    as_system_template: bool,
}

/// `POST /api/templates/import`
async fn import_templates(
    State(state): State<AppState>,
    Json(request): Json<ImportRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let value: serde_json::Value = serde_json::from_str(&request.json)
        .map_err(|error| ApiError::bad_request("IMPORT_INVALID_JSON", format!("导入内容不是合法的 JSON：{error}")))?;

    let result = state
        .templates
        .import_json(&value, request.as_system_template)
        .await
        .map_err(|error| template_write_error(&error.to_string()))?;

    Ok(Json(json!(result)))
}

/// `POST /api/templates/system/reset`
async fn reset_system_templates(
    State(state): State<AppState>,
) -> Result<Json<crate::templates::SystemSyncResult>, ApiError> {
    state
        .templates
        .reset_system_templates()
        .await
        .map(Json)
        .api("恢复系统预设失败")
}

/// `POST /api/templates/system/sync`
async fn sync_system_templates(
    State(state): State<AppState>,
) -> Result<Json<crate::templates::SystemSyncResult>, ApiError> {
    state
        .templates
        .initialize_system_templates()
        .await
        .map(Json)
        .api("同步系统模板失败")
}

/// 把模板服务的错误映射成合适的 HTTP 状态码：重名/系统模板不可删等是可预期的业务错误
fn template_write_error(message: &str) -> ApiError {
    let text = message.to_string();
    if text.contains("已存在") || text.contains("重名") {
        ApiError::conflict("TEMPLATE_NAME_EXISTS", text)
    } else if text.contains("系统预设") || text.contains("系统模板") {
        ApiError::forbidden("TEMPLATE_SYSTEM_READONLY", text)
    } else if text.contains("不存在") {
        ApiError::not_found("TEMPLATE_NOT_FOUND", text)
    } else if text.contains("不能为空") || text.contains("缺少") || text.contains("非法") {
        ApiError::bad_request("TEMPLATE_INVALID", text)
    } else {
        ApiError::internal("模板操作失败", text)
    }
}
