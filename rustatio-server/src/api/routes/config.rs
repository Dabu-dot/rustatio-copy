//! Default configuration endpoints.

use crate::services::scheduler::roll_and_apply_max_active_limits;
use axum::{
    extract::State,
    http::StatusCode,
    response::Response,
    routing::{delete, get, post, put},
    Json, Router,
};
use rustatio_core::{FakerConfig, PresetSettings};

use crate::api::{
    common::{ApiError, ApiSuccess, EmptyData},
    ServerState,
};
use crate::services::persistence::{DefaultPreset, MaxActiveSettings};

#[utoipa::path(
    get,
    path = "/config/default",
    tag = "config",
    summary = "Get default configuration",
    description = "Returns the default configuration used for new instances (e.g., from watch folder).",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Default configuration", body = ApiSuccess<Object>),
        (status = 401, description = "Unauthorized", body = ApiError)
    )
)]
pub async fn get_default_config(State(state): State<ServerState>) -> Response {
    let config = state.app.get_effective_default_config().await;
    ApiSuccess::response(config)
}

#[utoipa::path(
    put,
    path = "/config/default",
    tag = "config",
    summary = "Set default configuration",
    description = "Sets the default configuration to be used for new instances.",
    security(("bearer_auth" = [])),
    request_body(content = Object, description = "Preset settings in UI-friendly format"),
    responses(
        (status = 200, description = "Configuration saved", body = ApiSuccess<EmptyData>),
        (status = 401, description = "Unauthorized", body = ApiError),
        (status = 500, description = "Failed to save configuration", body = ApiError)
    )
)]
pub async fn set_default_config(
    State(state): State<ServerState>,
    Json(preset): Json<PresetSettings>,
) -> Response {
    let config: FakerConfig = preset.into();
    match state.app.set_default_config(Some(config)).await {
        Ok(()) => ApiSuccess::response(EmptyData {}),
        Err(e) => ApiError::response(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[utoipa::path(
    delete,
    path = "/config/default",
    tag = "config",
    summary = "Clear default configuration",
    description = "Clears the custom default configuration, reverting to built-in defaults.",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Configuration cleared", body = ApiSuccess<EmptyData>),
        (status = 401, description = "Unauthorized", body = ApiError),
        (status = 500, description = "Failed to clear configuration", body = ApiError)
    )
)]
pub async fn clear_default_config(State(state): State<ServerState>) -> Response {
    match state.app.set_default_config(None).await {
        Ok(()) => ApiSuccess::response(EmptyData {}),
        Err(e) => ApiError::response(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[utoipa::path(
    get,
    path = "/config/default-preset",
    tag = "config",
    summary = "Get default preset metadata",
    description = "Returns the preset metadata/settings used as the server default for new instances.",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Default preset", body = ApiSuccess<Object>),
        (status = 401, description = "Unauthorized", body = ApiError)
    )
)]
pub async fn get_default_preset(State(state): State<ServerState>) -> Response {
    ApiSuccess::response(state.app.get_default_preset().await)
}

#[utoipa::path(
    put,
    path = "/config/default-preset",
    tag = "config",
    summary = "Set default preset",
    description = "Sets the named preset used as the server default for new instances.",
    security(("bearer_auth" = [])),
    request_body(content = Object, description = "Preset metadata and settings"),
    responses(
        (status = 200, description = "Default preset saved", body = ApiSuccess<EmptyData>),
        (status = 401, description = "Unauthorized", body = ApiError),
        (status = 500, description = "Failed to save default preset", body = ApiError)
    )
)]
pub async fn set_default_preset(
    State(state): State<ServerState>,
    Json(preset): Json<DefaultPreset>,
) -> Response {
    match state.app.set_default_preset(Some(preset)).await {
        Ok(()) => ApiSuccess::response(EmptyData {}),
        Err(e) => ApiError::response(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[utoipa::path(
    delete,
    path = "/config/default-preset",
    tag = "config",
    summary = "Clear default preset",
    description = "Clears the named default preset metadata and server default config.",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Default preset cleared", body = ApiSuccess<EmptyData>),
        (status = 401, description = "Unauthorized", body = ApiError),
        (status = 500, description = "Failed to clear default preset", body = ApiError)
    )
)]
pub async fn clear_default_preset(State(state): State<ServerState>) -> Response {
    match state.app.set_default_preset(None).await {
        Ok(()) => ApiSuccess::response(EmptyData {}),
        Err(e) => ApiError::response(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[utoipa::path(
    get,
    path = "/config/max-active",
    tag = "config",
    summary = "Get max active instance settings",
    description = "Returns global and per-tracker max active instance limits and queue settings.",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Max active settings", body = ApiSuccess<Object>),
        (status = 401, description = "Unauthorized", body = ApiError)
    )
)]
pub async fn get_max_active_settings(State(state): State<ServerState>) -> Response {
    let settings = state.app.get_max_active_settings().await.unwrap_or_default();
    ApiSuccess::response(settings)
}

#[utoipa::path(
    put,
    path = "/config/max-active",
    tag = "config",
    summary = "Set max active instance settings",
    description = "Sets global and per-tracker max active instance limits.",
    security(("bearer_auth" = [])),
    request_body(content = Object, description = "Max active settings"),
    responses(
        (status = 200, description = "Max active settings saved", body = ApiSuccess<EmptyData>),
        (status = 401, description = "Unauthorized", body = ApiError),
        (status = 500, description = "Failed to save max active settings", body = ApiError)
    )
)]
pub async fn set_max_active_settings(
    State(state): State<ServerState>,
    Json(settings): Json<MaxActiveSettings>,
) -> Response {
    match state.app.set_max_active_settings(settings).await {
        Ok(()) => ApiSuccess::response(EmptyData {}),
        Err(e) => ApiError::response(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[utoipa::path(
    post,
    path = "/config/max-active/roll",
    tag = "config",
    summary = "Roll and apply max active limits now",
    description = "Immediately rolls new effective limits, resets the 24h timer, and runs reconciliation.",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "Limits rolled and applied", body = ApiSuccess<Object>),
        (status = 500, description = "Failed to roll limits", body = ApiError)
    )
)]
pub async fn roll_max_active_limits(State(state): State<ServerState>) -> Response {
    match roll_and_apply_max_active_limits(&state.app).await {
        Ok(settings) => ApiSuccess::response(settings),
        Err(e) => ApiError::response(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

pub fn router() -> Router<ServerState> {
    Router::new()
        .route("/config/default", get(get_default_config))
        .route("/config/default", put(set_default_config))
        .route("/config/default", delete(clear_default_config))
        .route("/config/default-preset", get(get_default_preset))
        .route("/config/default-preset", put(set_default_preset))
        .route("/config/default-preset", delete(clear_default_preset))
        .route("/config/max-active", get(get_max_active_settings))
        .route("/config/max-active", put(set_max_active_settings))
        .route("/config/max-active/roll", post(roll_max_active_limits))
}
