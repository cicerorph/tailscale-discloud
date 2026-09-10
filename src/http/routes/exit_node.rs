use axum::{http::StatusCode, response::Json};
use serde::{Deserialize, Serialize};

use crate::http::errors::ApiErrorResponse;
use crate::tailscale::localapi::models::Prefs;

#[derive(Debug, Serialize)]
pub struct ExitNodeStatusResponse {
    pub advertise_exit_node: bool,
    pub exit_node_allow_lan_access: bool,
    pub exit_node: String,
}

#[derive(Debug, Deserialize)]
pub struct ExitNodeUpdateRequest {
    pub advertise_exit_node: Option<bool>,
    pub exit_node_allow_lan_access: Option<bool>,
    pub exit_node: Option<String>,
}

#[tracing::instrument]
pub async fn get_exit_node_status() -> Result<Json<ExitNodeStatusResponse>, (StatusCode, Json<ApiErrorResponse>)> {
    tracing::debug!("GET /api/exit-node");

    let prefs = Prefs::get_current().await.map_err(|e| {
        tracing::error!("Failed to get Tailscale prefs: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorResponse {
                error: crate::http::errors::ApiMessage::new("api.error.tailscale_prefs_failed"),
            }),
        )
    })?;

    let resp = ExitNodeStatusResponse {
        advertise_exit_node: prefs.advertise_exit_node.unwrap_or(false),
        exit_node_allow_lan_access: prefs.exit_node_allow_lan_access.unwrap_or(false),
        exit_node: prefs.exit_node.unwrap_or_default(),
    };

    Ok(Json(resp))
}

#[tracing::instrument(skip(payload))]
pub async fn update_exit_node(
    Json(payload): Json<ExitNodeUpdateRequest>,
) -> Result<Json<ExitNodeStatusResponse>, (StatusCode, Json<ApiErrorResponse>)> {
    tracing::debug!(
        advertise_exit_node = ?payload.advertise_exit_node,
        exit_node_allow_lan_access = ?payload.exit_node_allow_lan_access,
        exit_node = ?payload.exit_node,
        "PUT /api/exit-node"
    );

    let mut prefs = Prefs::get_current().await.map_err(|e| {
        tracing::error!("Failed to get Tailscale prefs: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorResponse {
                error: crate::http::errors::ApiMessage::new("api.error.tailscale_prefs_failed"),
            }),
        )
    })?;

    if let Some(v) = payload.advertise_exit_node {
        prefs.advertise_exit_node = Some(v);
    }
    if let Some(v) = payload.exit_node_allow_lan_access {
        prefs.exit_node_allow_lan_access = Some(v);
    }
    if let Some(v) = payload.exit_node {
        prefs.exit_node = if v.is_empty() { None } else { Some(v) };
    }

    Prefs::update(prefs.clone()).await.map_err(|e| {
        tracing::error!("Failed to update Tailscale prefs: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiErrorResponse {
                error: crate::http::errors::ApiMessage::new("api.error.tailscale_prefs_update_failed"),
            }),
        )
    })?;

    tracing::info!("Exit node preferences updated");

    let resp = ExitNodeStatusResponse {
        advertise_exit_node: prefs.advertise_exit_node.unwrap_or(false),
        exit_node_allow_lan_access: prefs.exit_node_allow_lan_access.unwrap_or(false),
        exit_node: prefs.exit_node.unwrap_or_default(),
    };

    Ok(Json(resp))
}
