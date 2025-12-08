use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, error, warn};

use crate::AppState;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AuthInfo {
    pub sub: String,
    pub aud: String,
    pub exp: usize,
    pub iat: usize,
    pub iss: String,
}

#[derive(Debug, Deserialize)]
struct JwksResponse {
    keys: Vec<Jwk>,
}

#[derive(Debug, Deserialize)]
struct Jwk {
    kid: String,
    kty: String,
    n: String,
    e: String,
}

/// Fetch JWKS from LogTo and cache the keys
async fn fetch_jwks(jwks_uri: &str) -> Result<HashMap<String, DecodingKey>, String> {
    let response = reqwest::get(jwks_uri)
        .await
        .map_err(|e| format!("Failed to fetch JWKS: {}", e))?;

    let jwks: JwksResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse JWKS: {}", e))?;

    let mut keys = HashMap::new();
    for jwk in jwks.keys {
        if jwk.kty == "RSA" {
            let decoding_key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e)
                .map_err(|e| format!("Failed to create decoding key: {}", e))?;
            keys.insert(jwk.kid, decoding_key);
        }
    }

    Ok(keys)
}

/// JWT validation middleware
pub async fn jwt_middleware(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Check if JWT validation is bypassed
    if state.bypass_jwt_validation {
        debug!("JWT validation bypassed");
        // Create a dummy auth info for development
        let auth_info = AuthInfo {
            sub: "dev-user".to_string(),
            aud: "dev".to_string(),
            exp: 9999999999,
            iat: 0,
            iss: "dev".to_string(),
        };
        request.extensions_mut().insert(auth_info);
        return Ok(next.run(request).await);
    }

    // Extract the token from the Authorization header
    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));

    let token = match auth_header {
        Some(token) => token,
        None => {
            warn!("Missing or invalid Authorization header");
            return Err(StatusCode::UNAUTHORIZED);
        }
    };

    // Get JWKS URI and issuer from state
    let jwks_uri = match &state.logto_jwks_uri {
        Some(uri) => uri,
        None => {
            error!("LogTo JWKS URI not configured");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let issuer = match &state.logto_issuer {
        Some(iss) => iss,
        None => {
            error!("LogTo issuer not configured");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Decode the token header to get the key ID
    let header = match decode_header(token) {
        Ok(h) => h,
        Err(e) => {
            warn!("Failed to decode token header: {}", e);
            return Err(StatusCode::UNAUTHORIZED);
        }
    };

    let kid = match header.kid {
        Some(k) => k,
        None => {
            warn!("Token missing key ID");
            return Err(StatusCode::UNAUTHORIZED);
        }
    };

    // Fetch JWKS and get the appropriate key
    let keys = match fetch_jwks(jwks_uri).await {
        Ok(k) => k,
        Err(e) => {
            error!("Failed to fetch JWKS: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let decoding_key = match keys.get(&kid) {
        Some(k) => k,
        None => {
            warn!("Key ID not found in JWKS");
            return Err(StatusCode::UNAUTHORIZED);
        }
    };

    // Validate the token
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[issuer]);

    let token_data = match decode::<AuthInfo>(token, decoding_key, &validation) {
        Ok(data) => data,
        Err(e) => {
            warn!("Token validation failed: {}", e);
            return Err(StatusCode::UNAUTHORIZED);
        }
    };

    debug!("JWT validated for user: {}", token_data.claims.sub);

    // Insert the auth info into request extensions
    request.extensions_mut().insert(token_data.claims);

    Ok(next.run(request).await)
}
