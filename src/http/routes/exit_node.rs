use axum::{http::StatusCode, response::Json};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::http::errors::ApiErrorResponse;

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

/// Build an `ApiErrorResponse` with an error detail param.
fn api_err_detail(id: &str, detail: String) -> ApiErrorResponse {
    ApiErrorResponse {
        error: crate::http::errors::ApiMessage::with_params(
            id,
            std::collections::HashMap::from([("detail".to_string(), serde_json::Value::String(detail))]),
        ),
    }
}

/// Read the current Tailscale exit node state by parsing `tailscale get --set-flags`.
async fn get_current_flags() -> Result<String, String> {
    let output = Command::new("tailscale")
        .args(["get", "--set-flags"])
        .output()
        .await
        .map_err(|e| format!("Failed to run tailscale get: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("tailscale get failed: {stderr}"));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Parse flags string to extract exit node state.
fn parse_flags(flags: &str) -> ExitNodeStatusResponse {
    let mut advertise_exit_node = false;
    let mut exit_node_allow_lan_access = false;
    let mut exit_node = String::new();

    for flag in flags.split_whitespace() {
        match flag {
            "--advertise-exit-node" => advertise_exit_node = true,
            "--no-advertise-exit-node" => advertise_exit_node = false,
            "--exit-node-allow-lan-access" => exit_node_allow_lan_access = true,
            "--no-exit-node-allow-lan-access" => exit_node_allow_lan_access = false,
            _ => {
                if let Some(val) = flag.strip_prefix("--exit-node=") {
                    exit_node = val.to_string();
                }
            }
        }
    }

    ExitNodeStatusResponse {
        advertise_exit_node,
        exit_node_allow_lan_access,
        exit_node,
    }
}

/// Run `tailscale set` with the given args; returns stdout on success.
async fn tailscale_set(args: &[String]) -> Result<(), String> {
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    tracing::debug!("Running: tailscale set {}", arg_refs.join(" "));

    let output = Command::new("tailscale")
        .arg("set")
        .args(&arg_refs)
        .output()
        .await
        .map_err(|e| format!("Failed to run tailscale set: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("tailscale set failed: {stderr}"));
    }

    Ok(())
}

#[tracing::instrument]
pub async fn get_exit_node_status() -> Result<Json<ExitNodeStatusResponse>, (StatusCode, Json<ApiErrorResponse>)> {
    tracing::debug!("GET /api/exit-node");

    let flags = get_current_flags().await.map_err(|e| {
        tracing::error!("Failed to read Tailscale flags: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(api_err_detail("api.error.tailscale_prefs_failed", e)),
        )
    })?;

    let resp = parse_flags(&flags);
    tracing::debug!(?resp, "Exit node status parsed from CLI");
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

    // Read current state first
    let current_flags = get_current_flags().await.map_err(|e| {
        tracing::error!("Failed to read current Tailscale state: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(api_err_detail("api.error.tailscale_prefs_failed", e)),
        )
    })?;
    let current = parse_flags(&current_flags);

    // Merge requested changes with current state
    let desired_advertise = payload.advertise_exit_node.unwrap_or(current.advertise_exit_node);
    let desired_allow_lan = payload
        .exit_node_allow_lan_access
        .unwrap_or(current.exit_node_allow_lan_access);
    let desired_exit_node = match &payload.exit_node {
        Some(v) => v.trim().to_string(),
        None => current.exit_node.clone(),
    };

    // Build the tailscale set args
    let mut args: Vec<String> = Vec::new();
    args.push(if desired_advertise {
        "--advertise-exit-node".to_string()
    } else {
        "--no-advertise-exit-node".to_string()
    });
    args.push(if desired_allow_lan {
        "--exit-node-allow-lan-access".to_string()
    } else {
        "--no-exit-node-allow-lan-access".to_string()
    });
    if desired_exit_node.is_empty() {
        args.push("--exit-node=".to_string());
    } else {
        args.push(format!("--exit-node={desired_exit_node}"));
    }

    tailscale_set(&args).await.map_err(|e| {
        tracing::error!("Failed to update Tailscale exit node settings: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(api_err_detail("api.error.tailscale_prefs_update_failed", e)),
        )
    })?;

    // If we just started advertising as an exit node, run `tailscale up`
    // so the control plane picks up the change.
    if desired_advertise && !current.advertise_exit_node {
        tracing::info!("Running tailscale up to apply exit node advertisement...");
        let up_output = Command::new("tailscale").arg("up").output().await;
        match up_output {
            Ok(out) if !out.status.success() => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                tracing::warn!("tailscale up returned non-zero: {stderr}");
            }
            Err(e) => {
                tracing::warn!("Failed to run tailscale up: {e}");
            }
            _ => {
                tracing::info!("tailscale up completed successfully");
            }
        }
    }

    tracing::info!("Exit node preferences updated via tailscale set");

    // Read back the updated state
    let updated_flags = get_current_flags().await.map_err(|e| {
        tracing::error!("Failed to read updated Tailscale state: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(api_err_detail("api.error.tailscale_prefs_failed", e)),
        )
    })?;

    let resp = parse_flags(&updated_flags);
    Ok(Json(resp))
}