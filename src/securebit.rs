use anyhow::{bail, Context, Result};
use reqwest::Client;
use scraper::{Html, Selector};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

const BASE_URL: &str = "https://www.securebit.cloud";
const RPKI_URL: &str = "https://www.securebit.cloud/manage/internet/resources";
const API_URL: &str = "https://www.securebit.cloud/interface.exe?";
const USER_AGENT: &str = "Mozilla/5.0 (compatible; peerlab-gateway/0.1)";

/// How long the ROA cache is considered fresh before re-fetching from Securebit.
const CACHE_TTL: Duration = Duration::from_secs(300); // 5 minutes

#[derive(Clone)]
struct RoaCache {
    roas: Vec<Roa>,
    fetched_at: Instant,
}

#[derive(Clone)]
pub struct SecurebitClient {
    email: String,
    password: String,
    origin_asn: String,
    cache: Arc<RwLock<Option<RoaCache>>>,
}

#[derive(Debug, Clone)]
pub struct Roa {
    pub asn: String,
    pub prefix: String,
    pub max_len: String,
    pub row_id: String,
}

impl SecurebitClient {
    pub fn new(email: String, password: String, origin_asn: i32) -> Self {
        let origin_asn = format!("AS{origin_asn}");
        Self {
            email,
            password,
            origin_asn,
            cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Warm the ROA cache by fetching from Securebit. Call once after construction.
    pub async fn warm_cache(&self) {
        match self.fetch_and_cache().await {
            Ok(roas) => info!("ROA cache warmed with {} entries", roas.len()),
            Err(err) => warn!("Failed to warm ROA cache: {err}"),
        }
    }

    /// Build a fresh HTTP client with cookie store for a single session.
    fn build_http(&self) -> Result<Client> {
        Client::builder()
            .cookie_store(true)
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .context("failed to build HTTP client")
    }

    /// Login to Securebit and return the authenticated HTTP client.
    async fn login(&self) -> Result<Client> {
        let http = self.build_http()?;

        debug!("Logging in to Securebit");

        // GET the login page to pick up the PHPSESSID cookie
        http.get(BASE_URL)
            .send()
            .await
            .context("failed to load login page")?;

        // POST credentials
        let resp = http
            .post(BASE_URL)
            .form(&[
                ("mail", self.email.as_str()),
                ("pass", self.password.as_str()),
            ])
            .send()
            .await
            .context("login POST failed")?;

        let status = resp.status();
        let body = resp.text().await?;

        if body.contains("placeholder=\"Passwort\"") && body.contains("<form action=\"/\"") {
            bail!("Securebit login failed (status {status})");
        }

        debug!("Securebit login successful");
        Ok(http)
    }

    /// Fetch the RPKI page HTML using an authenticated client.
    async fn fetch_rpki_page(&self, http: &Client) -> Result<String> {
        let html = http
            .get(RPKI_URL)
            .send()
            .await
            .context("failed to load RPKI page")?
            .text()
            .await?;
        Ok(html)
    }

    /// Parse existing ROA rows from the RPKI page HTML.
    fn parse_roas(html: &str) -> Vec<Roa> {
        let doc = Html::parse_document(html);
        let row_sel = Selector::parse("#rpki tbody tr").unwrap();
        let action_sel = Selector::parse("input[name=\"action\"]").unwrap();
        let asn_sel = Selector::parse("input[name=\"asn\"]").unwrap();
        let prefix_sel = Selector::parse("input[name=\"prefix\"]").unwrap();
        let td_sel = Selector::parse("td").unwrap();

        let mut roas = Vec::new();

        for row in doc.select(&row_sel) {
            let action = row
                .select(&action_sel)
                .next()
                .and_then(|el| el.value().attr("value"));
            if action != Some("d") {
                continue;
            }

            let row_id = row.value().attr("id").unwrap_or("").to_string();
            let asn = row
                .select(&asn_sel)
                .next()
                .and_then(|el| el.value().attr("value"))
                .unwrap_or("")
                .to_string();
            let prefix = row
                .select(&prefix_sel)
                .next()
                .and_then(|el| el.value().attr("value"))
                .unwrap_or("")
                .to_string();
            let max_len = row
                .select(&td_sel)
                .nth(2)
                .map(|td| td.text().collect::<String>().trim().to_string())
                .unwrap_or_else(|| "?".to_string());

            roas.push(Roa {
                asn,
                prefix,
                max_len,
                row_id,
            });
        }

        roas
    }

    /// Extract the row ID of the "add" row (action=c).
    fn parse_add_row_id(html: &str) -> Option<String> {
        let doc = Html::parse_document(html);
        let row_sel = Selector::parse("#rpki tbody tr").unwrap();
        let action_sel = Selector::parse("input[name=\"action\"]").unwrap();

        for row in doc.select(&row_sel) {
            let action = row
                .select(&action_sel)
                .next()
                .and_then(|el| el.value().attr("value"));
            if action == Some("c") {
                return row.value().attr("id").map(|s| s.to_string());
            }
        }
        None
    }

    /// Call the Save AJAX endpoint (mimics the JS Save() function).
    async fn save_row(
        http: &Client,
        action: &str,
        asn: &str,
        prefix: &str,
        row_id: &str,
    ) -> Result<String> {
        let resp = http
            .post(API_URL)
            .form(&[
                ("action", action),
                ("asn", asn),
                ("prefix", prefix),
                ("path", "/manage/internet/resources"),
                ("id", row_id),
                ("language", "de"),
            ])
            .send()
            .await
            .context("Save AJAX call failed")?;

        let body = resp.text().await?;
        debug!("Save response: {body}");
        Ok(body)
    }

    // -- Cache helpers --

    /// Update the cache with a fresh ROA list.
    async fn update_cache(&self, roas: Vec<Roa>) {
        let mut guard = self.cache.write().await;
        *guard = Some(RoaCache {
            roas,
            fetched_at: Instant::now(),
        });
    }

    /// Fetch ROAs from Securebit and update the cache.
    async fn fetch_and_cache(&self) -> Result<Vec<Roa>> {
        let http = self.login().await?;
        let html = self.fetch_rpki_page(&http).await?;
        let roas = Self::parse_roas(&html);
        self.update_cache(roas.clone()).await;
        Ok(roas)
    }

    // -- Public API --

    /// List all current ROAs. Serves from cache if fresh, otherwise fetches from Securebit.
    pub async fn list_roas(&self) -> Result<Vec<Roa>> {
        {
            let guard = self.cache.read().await;
            if let Some(cached) = guard.as_ref() {
                if cached.fetched_at.elapsed() < CACHE_TTL {
                    debug!("Serving ROA list from cache");
                    return Ok(cached.roas.clone());
                }
            }
        }

        debug!("ROA cache miss or expired, fetching from Securebit");
        self.fetch_and_cache().await
    }

    /// Add a ROA for the given prefix with the configured origin ASN.
    /// Returns Ok(true) if added, Ok(false) if already exists.
    pub async fn add_roa(&self, prefix: &str) -> Result<bool> {
        let http = self.login().await?;
        let html = self.fetch_rpki_page(&http).await?;

        let existing = Self::parse_roas(&html);
        if existing
            .iter()
            .any(|r| r.prefix == prefix && r.asn == self.origin_asn)
        {
            info!("ROA already exists for {prefix} {}", self.origin_asn);
            self.update_cache(existing).await;
            return Ok(false);
        }

        let add_row_id =
            Self::parse_add_row_id(&html).context("add row not found in RPKI table")?;

        info!(
            "Adding ROA: {prefix} max-len=48 {}",
            self.origin_asn
        );
        Self::save_row(&http, "c", &self.origin_asn, prefix, &add_row_id).await?;

        // Verify and update cache
        let html = self.fetch_rpki_page(&http).await?;
        let roas = Self::parse_roas(&html);
        let success = roas.iter().any(|r| r.prefix == prefix);
        self.update_cache(roas).await;

        if success {
            info!("ROA added successfully for {prefix}");
            Ok(true)
        } else {
            warn!("ROA may not have been added for {prefix}");
            bail!("ROA add verification failed for {prefix}");
        }
    }

    /// Remove the ROA for the given prefix.
    /// Returns Ok(true) if removed, Ok(false) if not found.
    pub async fn remove_roa(&self, prefix: &str) -> Result<bool> {
        let http = self.login().await?;
        let html = self.fetch_rpki_page(&http).await?;

        let existing = Self::parse_roas(&html);
        let target = match existing.iter().find(|r| r.prefix == prefix) {
            Some(roa) => roa.clone(),
            None => {
                info!("ROA not found for {prefix}, nothing to remove");
                self.update_cache(existing).await;
                return Ok(false);
            }
        };

        info!("Removing ROA: {} {prefix}", target.asn);
        Self::save_row(&http, "d", &target.asn, prefix, &target.row_id).await?;

        // Verify and update cache
        let html = self.fetch_rpki_page(&http).await?;
        let roas = Self::parse_roas(&html);
        let still_present = roas.iter().any(|r| r.prefix == prefix);
        self.update_cache(roas).await;

        if still_present {
            warn!("ROA may not have been removed for {prefix}");
            bail!("ROA remove verification failed for {prefix}");
        } else {
            info!("ROA removed successfully for {prefix}");
            Ok(true)
        }
    }

    /// Check if a ROA exists for the given prefix (uses cache).
    pub async fn has_roa(&self, prefix: &str) -> Result<bool> {
        let roas = self.list_roas().await?;
        Ok(roas.iter().any(|r| r.prefix == prefix))
    }

    /// Ensure a ROA exists for the given prefix (add if missing, no-op if present).
    pub async fn ensure_roa(&self, prefix: &str) -> Result<()> {
        self.add_roa(prefix).await?;
        Ok(())
    }
}
