pub mod database;
pub mod jwt;
pub mod prefix_pool;

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use hex;
use ipnet::Ipv6Net;
use sha2::{Digest, Sha256};
use std::str::FromStr;
use tower_http::trace::TraceLayer;
use tracing::{debug, error, warn};

use database::Database;
use prefix_pool::PrefixPool;

#[derive(Clone)]
pub struct AppState {
    pub database: Database,
    pub prefix_pool: PrefixPool,
    pub logto_jwks_uri: Option<String>,
    pub logto_issuer: Option<String>,
    pub bypass_jwt_validation: bool,
}

// Client-facing API (requires JWT authentication)
pub fn create_client_app(state: AppState) -> Router {
    let protected_routes = Router::new()
        .route("/user/info", get(get_user_info))
        .route("/user/asn", post(request_asn))
        .route("/user/prefix", post(request_prefix))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            jwt::jwt_middleware,
        ));

    Router::new()
        .merge(protected_routes)
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

// Service-facing API (for downstream services to query mappings)
pub fn create_service_app(state: AppState) -> Router {
    Router::new()
        .route("/mappings", get(get_all_mappings))
        .route("/mappings/:user_hash", get(get_user_mapping))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

// Combined app with both client and service endpoints
pub fn create_app(state: AppState) -> Router {
    let client_router = create_client_app(state.clone());
    let service_router = create_service_app(state);

    Router::new()
        .nest("/api", client_router)
        .nest("/service", service_router)
}

/// Compute a consistent hash for a user identifier
pub fn hash_user_identifier(user_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(user_id.as_bytes());
    hex::encode(hasher.finalize())
}

// Request/Response types
#[derive(serde::Deserialize)]
struct RequestAsnRequest {
    asn: i32,
}

#[derive(serde::Deserialize)]
struct RequestPrefixRequest {
    duration_hours: i32,
}

#[derive(serde::Serialize)]
struct UserInfoResponse {
    user_hash: String,
    asn: Option<i32>,
    active_leases: Vec<PrefixLeaseResponse>,
}

#[derive(serde::Serialize)]
struct PrefixLeaseResponse {
    prefix: String,
    start_time: String,
    end_time: String,
}

#[derive(serde::Serialize)]
struct RequestAsnResponse {
    asn: i32,
    message: String,
}

#[derive(serde::Serialize)]
struct RequestPrefixResponse {
    prefix: String,
    start_time: String,
    end_time: String,
    message: String,
}

#[derive(serde::Serialize)]
struct UserMappingResponse {
    user_hash: String,
    asn: i32,
    prefixes: Vec<String>,
}

#[derive(serde::Serialize)]
struct AllMappingsResponse {
    mappings: Vec<UserMappingResponse>,
}

// Handler implementations

/// Get user information (ASN and active leases)
async fn get_user_info(
    Extension(auth_info): Extension<jwt::AuthInfo>,
    State(state): State<AppState>,
) -> Result<Json<UserInfoResponse>, (StatusCode, Json<serde_json::Value>)> {
    let user_hash = hash_user_identifier(&auth_info.sub);

    match state.database.get_user_info(&user_hash).await {
        Ok(Some((asn_mapping, leases))) => {
            let active_leases = leases
                .into_iter()
                .map(|lease| PrefixLeaseResponse {
                    prefix: lease.prefix,
                    start_time: lease.start_time.to_rfc3339(),
                    end_time: lease.end_time.to_rfc3339(),
                })
                .collect();

            Ok(Json(UserInfoResponse {
                user_hash,
                asn: asn_mapping.map(|m| m.asn),
                active_leases,
            }))
        }
        Ok(None) => Ok(Json(UserInfoResponse {
            user_hash,
            asn: None,
            active_leases: Vec::new(),
        })),
        Err(err) => {
            error!("Failed to get user info: {}", err);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": 500,
                    "message": "Failed to retrieve user information"
                })),
            ))
        }
    }
}

/// Request an ASN for the user
async fn request_asn(
    Extension(auth_info): Extension<jwt::AuthInfo>,
    State(state): State<AppState>,
    Json(request): Json<RequestAsnRequest>,
) -> Result<Json<RequestAsnResponse>, (StatusCode, Json<serde_json::Value>)> {
    let user_hash = hash_user_identifier(&auth_info.sub);

    // Check if user already has an ASN
    match state.database.get_user_asn(&user_hash).await {
        Ok(Some(existing)) => {
            debug!("User {} already has ASN {}", user_hash, existing.asn);
            return Ok(Json(RequestAsnResponse {
                asn: existing.asn,
                message: "ASN already assigned".to_string(),
            }));
        }
        Ok(None) => {}
        Err(err) => {
            error!("Failed to check existing ASN: {}", err);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": 500,
                    "message": "Failed to check ASN assignment"
                })),
            ));
        }
    }

    // Check if requested ASN is already assigned
    match state.database.is_asn_assigned(request.asn).await {
        Ok(true) => {
            warn!("ASN {} is already assigned", request.asn);
            return Err((
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": 409,
                    "message": format!("ASN {} is already assigned to another user", request.asn)
                })),
            ));
        }
        Ok(false) => {}
        Err(err) => {
            error!("Failed to check ASN availability: {}", err);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": 500,
                    "message": "Failed to check ASN availability"
                })),
            ));
        }
    }

    // Assign the ASN
    match state
        .database
        .get_or_create_user_asn(&user_hash, request.asn)
        .await
    {
        Ok(mapping) => {
            debug!("Assigned ASN {} to user {}", mapping.asn, user_hash);
            Ok(Json(RequestAsnResponse {
                asn: mapping.asn,
                message: "ASN assigned successfully".to_string(),
            }))
        }
        Err(err) => {
            error!("Failed to assign ASN: {}", err);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": 500,
                    "message": "Failed to assign ASN"
                })),
            ))
        }
    }
}

/// Request a prefix lease for the user
async fn request_prefix(
    Extension(auth_info): Extension<jwt::AuthInfo>,
    State(state): State<AppState>,
    Json(request): Json<RequestPrefixRequest>,
) -> Result<Json<RequestPrefixResponse>, (StatusCode, Json<serde_json::Value>)> {
    let user_hash = hash_user_identifier(&auth_info.sub);

    // Validate duration (e.g., max 24 hours)
    if request.duration_hours < 1 || request.duration_hours > 24 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": 400,
                "message": "Duration must be between 1 and 24 hours"
            })),
        ));
    }

    // Get all currently leased prefixes
    let active_leases = match state.database.get_all_active_leases().await {
        Ok(leases) => leases,
        Err(err) => {
            error!("Failed to get active leases: {}", err);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": 500,
                    "message": "Failed to check available prefixes"
                })),
            ));
        }
    };

    let leased_prefixes: Vec<Ipv6Net> = active_leases
        .iter()
        .filter_map(|lease| Ipv6Net::from_str(&lease.prefix).ok())
        .collect();

    // Find an available prefix
    let available_prefix = match state.prefix_pool.find_available_prefix(&leased_prefixes) {
        Some(prefix) => prefix,
        None => {
            warn!("No available prefixes in the pool");
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": 503,
                    "message": "No available prefixes at this time"
                })),
            ));
        }
    };

    // Create the lease
    match state
        .database
        .create_prefix_lease(&user_hash, &available_prefix, request.duration_hours)
        .await
    {
        Ok(lease) => {
            debug!(
                "Created prefix lease {} for user {} until {}",
                lease.prefix, user_hash, lease.end_time
            );
            Ok(Json(RequestPrefixResponse {
                prefix: lease.prefix,
                start_time: lease.start_time.to_rfc3339(),
                end_time: lease.end_time.to_rfc3339(),
                message: "Prefix leased successfully".to_string(),
            }))
        }
        Err(err) => {
            error!("Failed to create prefix lease: {}", err);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": 500,
                    "message": "Failed to create prefix lease"
                })),
            ))
        }
    }
}

/// Get all user mappings (for downstream services)
async fn get_all_mappings(
    State(state): State<AppState>,
) -> Result<Json<AllMappingsResponse>, (StatusCode, Json<serde_json::Value>)> {
    match state.database.get_all_user_mappings().await {
        Ok(mappings) => {
            let response_mappings = mappings
                .into_iter()
                .map(|(asn_mapping, leases)| UserMappingResponse {
                    user_hash: asn_mapping.user_hash,
                    asn: asn_mapping.asn,
                    prefixes: leases.into_iter().map(|l| l.prefix).collect(),
                })
                .collect();

            Ok(Json(AllMappingsResponse {
                mappings: response_mappings,
            }))
        }
        Err(err) => {
            error!("Failed to get all mappings: {}", err);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": 500,
                    "message": "Failed to retrieve mappings"
                })),
            ))
        }
    }
}

/// Get mapping for a specific user (for downstream services)
async fn get_user_mapping(
    State(state): State<AppState>,
    axum::extract::Path(user_hash): axum::extract::Path<String>,
) -> Result<Json<UserMappingResponse>, (StatusCode, Json<serde_json::Value>)> {
    match state.database.get_user_info(&user_hash).await {
        Ok(Some((Some(asn_mapping), leases))) => {
            Ok(Json(UserMappingResponse {
                user_hash: asn_mapping.user_hash,
                asn: asn_mapping.asn,
                prefixes: leases.into_iter().map(|l| l.prefix).collect(),
            }))
        }
        Ok(Some((None, _))) => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": 404,
                "message": "User has no ASN assigned"
            })),
        )),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": 404,
                "message": "User not found"
            })),
        )),
        Err(err) => {
            error!("Failed to get user mapping: {}", err);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": 500,
                    "message": "Failed to retrieve user mapping"
                })),
            ))
        }
    }
}
