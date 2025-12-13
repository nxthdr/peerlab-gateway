pub mod agent;
pub mod auth0;
pub mod database;
pub mod jwt;
pub mod pool_asns;
pub mod pool_prefixes;

use axum::{
    Router,
    extract::{Extension, Request, State},
    http::StatusCode,
    middleware::Next,
    response::Json,
    response::Response,
    routing::{get, post},
};
use hex;
use ipnet::Ipv6Net;
use sha2::{Digest, Sha256};
use std::str::FromStr;
use tower_http::trace::TraceLayer;
use tracing::{debug, error, warn};

use agent::AgentStore;
use database::Database;
use pool_asns::AsnPool;
use pool_prefixes::PrefixPool;

#[derive(Clone)]
pub struct AppState {
    pub agent_store: AgentStore,
    pub agent_key: String,
    pub database: Database,
    pub asn_pool: AsnPool,
    pub prefix_pool: PrefixPool,
    pub auth0_jwks_uri: Option<String>,
    pub auth0_issuer: Option<String>,
    pub auth0_management_api: Option<String>,
    pub auth0_m2m_app_id: Option<String>,
    pub auth0_m2m_app_secret: Option<String>,
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
// Requires agent authentication
pub fn create_service_app(state: AppState) -> Router {
    Router::new()
        .route("/mappings", get(get_all_mappings))
        .route("/mappings/{user_hash}", get(get_user_mapping))
        .with_state(state.clone())
        .layer(axum::middleware::from_fn_with_state(
            state,
            validate_agent_key,
        ))
        .layer(TraceLayer::new_for_http())
}

// API key validation middleware
async fn validate_agent_key(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));

    match auth_header {
        Some(key) if key == state.agent_key => Ok(next.run(request).await),
        _ => {
            warn!("Unauthorized access attempt to service API");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
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

// Request/Response types (ASN request no longer needs a body)

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
    user_id: String,
    email: Option<String>,
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

/// Request an ASN for the user (auto-assigned from pool)
async fn request_asn(
    Extension(auth_info): Extension<jwt::AuthInfo>,
    State(state): State<AppState>,
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

    // Find an available ASN from the pool (checks database for assigned ASNs)
    let available_asn = match state.asn_pool.find_available_asn(&state.database).await {
        Ok(Some(asn)) => asn,
        Ok(None) => {
            warn!("No available ASNs in the pool");
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": 503,
                    "message": "No available ASNs at this time"
                })),
            ));
        }
        Err(err) => {
            error!("Failed to find available ASN: {}", err);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": 500,
                    "message": "Failed to check ASN availability"
                })),
            ));
        }
    };

    // Assign the ASN with user_id
    match state
        .database
        .get_or_create_user_asn(&user_hash, Some(&auth_info.sub), available_asn)
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
            let mut response_mappings = Vec::new();

            for (asn_mapping, leases) in mappings {
                // Fetch email from Auth0 if we have the necessary configuration
                let email = if let (Some(user_id), Some(api_url), Some(app_id), Some(app_secret)) = (
                    &asn_mapping.user_id,
                    &state.auth0_management_api,
                    &state.auth0_m2m_app_id,
                    &state.auth0_m2m_app_secret,
                ) {
                    match auth0::get_user_email(user_id, api_url, app_id, app_secret).await {
                        Ok(email) => email,
                        Err(e) => {
                            warn!("Failed to fetch email for user {}: {}", user_id, e);
                            None
                        }
                    }
                } else {
                    None
                };

                response_mappings.push(UserMappingResponse {
                    user_hash: asn_mapping.user_hash.clone(),
                    user_id: asn_mapping.user_id.clone().unwrap_or_default(),
                    email,
                    asn: asn_mapping.asn,
                    prefixes: leases.into_iter().map(|l| l.prefix).collect(),
                });
            }

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
            // Fetch email from Auth0 if we have the necessary configuration
            let email = if let (Some(user_id), Some(api_url), Some(app_id), Some(app_secret)) = (
                &asn_mapping.user_id,
                &state.auth0_management_api,
                &state.auth0_m2m_app_id,
                &state.auth0_m2m_app_secret,
            ) {
                match auth0::get_user_email(user_id, api_url, app_id, app_secret).await {
                    Ok(email) => email,
                    Err(e) => {
                        warn!("Failed to fetch email for user {}: {}", user_id, e);
                        None
                    }
                }
            } else {
                None
            };

            Ok(Json(UserMappingResponse {
                user_hash: asn_mapping.user_hash.clone(),
                user_id: asn_mapping.user_id.clone().unwrap_or_default(),
                email,
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
