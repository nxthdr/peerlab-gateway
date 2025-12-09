use axum::{
    Json,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, fmt, sync::Arc, time::Duration};
use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::AppState;

// JWT configuration functions to get values from AppState
pub fn jwks_uri(state: &AppState) -> Result<String, AuthorizationError> {
    state.logto_jwks_uri.clone().ok_or_else(|| {
        AuthorizationError::with_status("LOGTO_JWKS_URI is not configured", 500)
    })
}

pub fn issuer(state: &AppState) -> Result<String, AuthorizationError> {
    state.logto_issuer.clone().ok_or_else(|| {
        AuthorizationError::with_status("LOGTO_ISSUER is not configured", 500)
    })
}

// For configuring HTTP client with reasonable timeouts
fn create_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_else(|e| {
            warn!("Failed to build HTTP client: {}, using default instead", e);
            reqwest::Client::new()
        })
}

// A cached JWKS validator that's shared across requests
static JWKS_CACHE: Lazy<Arc<RwLock<Option<JwtValidator>>>> =
    Lazy::new(|| Arc::new(RwLock::new(None)));

// How long to cache JWKS before refreshing (12 hours)
const JWKS_CACHE_DURATION: Duration = Duration::from_secs(12 * 60 * 60);

// Timestamp of last JWKS refresh
static LAST_JWKS_REFRESH: Lazy<Arc<RwLock<Option<std::time::Instant>>>> =
    Lazy::new(|| Arc::new(RwLock::new(None)));

// No longer needed - we get the bypass flag directly from AppState

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthInfo {
    pub sub: String,
    pub email: Option<String>,
    pub client_id: Option<String>,
    pub organization_id: Option<String>,
    pub scopes: Vec<String>,
    pub audience: Vec<String>,
}

impl AuthInfo {
    pub fn new(
        sub: String,
        email: Option<String>,
        client_id: Option<String>,
        organization_id: Option<String>,
        scopes: Vec<String>,
        audience: Vec<String>,
    ) -> Self {
        Self {
            sub,
            email,
            client_id,
            organization_id,
            scopes,
            audience,
        }
    }
}

#[derive(Debug)]
pub struct AuthorizationError {
    pub message: String,
    pub status_code: u16,
}

impl AuthorizationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status_code: 403,
        }
    }

    pub fn with_status(message: impl Into<String>, status_code: u16) -> Self {
        Self {
            message: message.into(),
            status_code,
        }
    }
}

impl fmt::Display for AuthorizationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AuthorizationError {}

impl IntoResponse for AuthorizationError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status_code).unwrap_or(StatusCode::FORBIDDEN);
        (status, Json(json!({ "error": self.message }))).into_response()
    }
}

pub fn extract_bearer_token(authorization: Option<&str>) -> Result<&str, AuthorizationError> {
    let auth_header = authorization
        .ok_or_else(|| AuthorizationError::with_status("Authorization header is missing", 401))?;

    if !auth_header.starts_with("Bearer ") {
        return Err(AuthorizationError::with_status(
            "Authorization header must start with \"Bearer \"",
            401,
        ));
    }

    Ok(&auth_header[7..]) // Remove 'Bearer ' prefix
}

#[derive(Clone)]
pub struct JwtValidator {
    jwks: HashMap<String, DecodingKey>,
}

impl JwtValidator {
    pub async fn new(state: &AppState) -> Result<Self, AuthorizationError> {
        let jwks = Self::fetch_jwks(state).await?;
        Ok(Self { jwks })
    }

    pub async fn get_or_create(state: &AppState) -> Result<Self, AuthorizationError> {
        // Check if we have a cached validator that's still fresh
        let should_refresh = {
            let last_refresh = LAST_JWKS_REFRESH.read().await;
            match *last_refresh {
                Some(time) => time.elapsed() > JWKS_CACHE_DURATION,
                None => true,
            }
        };

        if should_refresh {
            // Need to refresh the JWKS
            debug!("JWKS cache expired or not initialized, fetching new keys");
            let new_validator = Self::new(state).await?;

            // Update the cache
            {
                let mut cache = JWKS_CACHE.write().await;
                *cache = Some(new_validator.clone());

                let mut last_refresh = LAST_JWKS_REFRESH.write().await;
                *last_refresh = Some(std::time::Instant::now());
            }

            Ok(new_validator)
        } else {
            // Use the cached validator
            let cache = JWKS_CACHE.read().await;
            match &*cache {
                Some(validator) => Ok(validator.clone()),
                None => {
                    // Should never happen, but just in case
                    warn!("JWKS cache inconsistency, fetching new keys");
                    Self::new(state).await
                }
            }
        }
    }

    async fn fetch_jwks(state: &AppState) -> Result<HashMap<String, DecodingKey>, AuthorizationError> {
        let jwks_uri = jwks_uri(state)?;
        let client = create_http_client();

        debug!("Fetching JWKS from {}", jwks_uri);

        // Simple fetch with basic error handling
        let response = client.get(&jwks_uri).send().await.map_err(|e| {
            warn!("JWKS fetch error: {}", e);
            AuthorizationError::with_status(
                format!("Failed to fetch JWKS from {}: {}", jwks_uri, e),
                401,
            )
        })?;

        if !response.status().is_success() {
            warn!("JWKS request failed with status {}", response.status());
            return Err(AuthorizationError::with_status(
                format!("JWKS request failed with status: {}", response.status()),
                401,
            ));
        }

        let jwks = response.json::<Value>().await.map_err(|e| {
            warn!("Failed to parse JWKS: {}", e);
            AuthorizationError::with_status(format!("Failed to parse JWKS: {}", e), 401)
        })?;

        debug!("Successfully fetched JWKS");
        Ok(Self::parse_jwks(jwks)?)
    }

    fn parse_jwks(jwks: Value) -> Result<HashMap<String, DecodingKey>, AuthorizationError> {
        let mut keys: HashMap<String, DecodingKey> = HashMap::new();

        if let Some(keys_array) = jwks["keys"].as_array() {
            for key in keys_array {
                let kid = key["kid"].as_str();
                let kty = key["kty"].as_str();

                if kid.is_none() || kty.is_none() {
                    continue; // Skip keys missing required fields
                }

                let kid = kid.unwrap();
                let kty = kty.unwrap();

                match kty {
                    // Handle RSA keys
                    "RSA" => {
                        if let (Some(n), Some(e)) = (key["n"].as_str(), key["e"].as_str()) {
                            if let Ok(decoding_key) = DecodingKey::from_rsa_components(n, e) {
                                keys.insert(kid.to_string(), decoding_key);
                            }
                        }
                    }
                    // Handle EC (Elliptic Curve) keys
                    "EC" => {
                        if let (Some(x), Some(y), Some(_crv)) =
                            (key["x"].as_str(), key["y"].as_str(), key["crv"].as_str())
                        {
                            // For EC keys, we need to convert x and y to a single point
                            if let Ok(decoding_key) = DecodingKey::from_ec_components(x, y) {
                                keys.insert(kid.to_string(), decoding_key);
                            }
                        }
                    }
                    // If we have other key types in the future, we can add them here
                    _ => {} // Ignore unsupported key types
                }
            }
        }

        if keys.is_empty() {
            return Err(AuthorizationError::with_status(
                "No valid keys found in JWKS",
                401,
            ));
        }

        Ok(keys)
    }

    pub fn validate_jwt(&self, state: &AppState, token: &str) -> Result<AuthInfo, AuthorizationError> {
        let header = decode_header(token).map_err(|e| {
            AuthorizationError::with_status(format!("Invalid token header: {}", e), 401)
        })?;

        let kid = header
            .kid
            .ok_or_else(|| AuthorizationError::with_status("Token missing kid claim", 401))?;

        let key = self
            .jwks
            .get(&kid)
            .ok_or_else(|| AuthorizationError::with_status("Unknown key ID", 401))?;

        // Determine the correct algorithm based on the token header
        let algorithm = match header.alg {
            // RSA algorithms
            jsonwebtoken::Algorithm::RS256 => Algorithm::RS256,
            jsonwebtoken::Algorithm::RS384 => Algorithm::RS384,
            jsonwebtoken::Algorithm::RS512 => Algorithm::RS512,

            // EC algorithms
            jsonwebtoken::Algorithm::ES256 => Algorithm::ES256,
            jsonwebtoken::Algorithm::ES384 => Algorithm::ES384,

            // Default to RS256 for other algorithms
            _ => {
                return Err(AuthorizationError::with_status(
                    format!("Unsupported algorithm: {:?}", header.alg),
                    401,
                ));
            }
        };

        let mut validation = Validation::new(algorithm);
        validation.set_issuer(&[&issuer(state)?]);
        validation.validate_aud = false; // We'll verify audience manually

        let token_data = decode::<Value>(token, key, &validation)
            .map_err(|e| AuthorizationError::with_status(format!("Invalid token: {}", e), 401))?;

        let claims = token_data.claims;

        // Here we can verify specific claims like audience, scopes, etc.
        // For simplicity, we'll do minimal validation

        Ok(self.create_auth_info(claims))
    }

    fn create_auth_info(&self, claims: Value) -> AuthInfo {
        let scopes = claims["scope"]
            .as_str()
            .map(|s| s.split(' ').map(|s| s.to_string()).collect())
            .unwrap_or_default();

        let audience = match &claims["aud"] {
            Value::Array(arr) => arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
            Value::String(s) => vec![s.clone()],
            _ => vec![],
        };

        AuthInfo::new(
            claims["sub"].as_str().unwrap_or_default().to_string(),
            claims["email"].as_str().map(|s| s.to_string()),
            claims["client_id"].as_str().map(|s| s.to_string()),
            claims["organization_id"].as_str().map(|s| s.to_string()),
            scopes,
            audience,
        )
    }
}

// JWT middleware for validating tokens
pub async fn jwt_middleware(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, AuthorizationError> {
    // Check if we should bypass JWT validation (for development/testing)
    if state.bypass_jwt_validation {
        // Create dummy auth info for development/testing
        let dummy_auth = AuthInfo::new(
            "test-user-id".to_string(),
            Some("test@example.com".to_string()),
            Some("test-client".to_string()),
            None,
            vec!["api:read".to_string(), "api:write".to_string()],
            vec!["https://api.example.com".to_string()],
        );

        // Log that we're bypassing JWT validation
        warn!("⚠️ BYPASSING JWT VALIDATION - For development/testing only!");

        // Store dummy auth info in request extensions
        request.extensions_mut().insert(dummy_auth);

        return Ok(next.run(request).await);
    }

    // Normal JWT validation path using the cached validator
    debug!("Validating JWT token");
    let validator = JwtValidator::get_or_create(&state).await?;

    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok());

    let token = extract_bearer_token(auth_header)?;
    let auth_info = validator.validate_jwt(&state, token)?;

    // Store auth info in request extensions for handlers to use
    request.extensions_mut().insert(auth_info);

    Ok(next.run(request).await)
}
