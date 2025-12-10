use serde::{Deserialize, Serialize};
use tracing::{debug, error};

#[derive(Debug, Serialize)]
#[allow(dead_code)]
struct TokenRequest {
    grant_type: String,
    resource: String,
    scope: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[allow(dead_code)]
    expires_in: u64,
    #[allow(dead_code)]
    token_type: String,
}

#[derive(Debug, Deserialize)]
struct LogtoUser {
    #[allow(dead_code)]
    pub id: String,
    #[serde(rename = "primaryEmail")]
    pub primary_email: Option<String>,
}

/// Fetch user email from Logto Management API
pub async fn get_user_email(
    user_id: &str,
    management_api_url: &str,
    app_id: &str,
    app_secret: &str,
) -> Result<Option<String>, String> {
    // Get M2M access token
    let token = get_m2m_token(management_api_url, app_id, app_secret).await?;

    // Fetch user details
    let client = reqwest::Client::new();
    let user_url = format!("{}/api/users/{}", management_api_url, user_id);

    debug!("Fetching user details from Logto: {}", user_url);

    let response = client
        .get(&user_url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .map_err(|e| format!("Failed to fetch user from Logto: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        error!("Logto API returned error {}: {}", status, error_text);
        return Err(format!("Logto API error: {} - {}", status, error_text));
    }

    let user: LogtoUser = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse Logto user response: {}", e))?;

    Ok(user.primary_email)
}

/// Get M2M access token for Logto Management API
async fn get_m2m_token(
    management_api_url: &str,
    app_id: &str,
    app_secret: &str,
) -> Result<String, String> {
    let client = reqwest::Client::new();
    // Extract base URL from management API URL (remove /api if present)
    let base_url = management_api_url
        .trim_end_matches("/api")
        .trim_end_matches('/');
    let token_url = format!("{}/oidc/token", base_url);

    debug!("Requesting M2M token from Logto: {}", token_url);

    let params = [
        ("grant_type", "client_credentials"),
        ("resource", &format!("{}/api", base_url)),
        ("scope", "all"),
    ];

    let response = client
        .post(&token_url)
        .basic_auth(app_id, Some(app_secret))
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Failed to request M2M token: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        error!(
            "Logto token endpoint returned error {}: {}",
            status, error_text
        );
        return Err(format!(
            "Failed to get M2M token: {} - {}",
            status, error_text
        ));
    }

    let token_response: TokenResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse token response: {}", e))?;

    debug!("Successfully obtained M2M token");
    Ok(token_response.access_token)
}
