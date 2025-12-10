use anyhow::Result;
use clap::Parser;
use clap_verbosity_flag::{InfoLevel, Verbosity};
use std::net::SocketAddr;
use tracing::{error, info, warn};

use peerlab_gateway::{
    AppState,
    agent::AgentStore,
    create_app,
    database::{Database, DatabaseConfig},
    pool_asns::AsnPool,
    pool_prefixes::PrefixPool,
};

/// Command line arguments for the gateway
#[derive(Parser, Debug)]
#[command(author, version, about)]
pub struct Cli {
    /// API listen address (e.g. 0.0.0.0:8080 or [::]:8080)
    #[arg(long = "address", default_value = "0.0.0.0:8080")]
    pub address: String,

    /// PostgreSQL database URL
    #[arg(
        long = "database-url",
        default_value = "postgresql://localhost/peerlab_gateway"
    )]
    pub database_url: String,

    /// Path to prefix pool file (one /48 prefix per line)
    #[arg(long = "prefix-pool-file", default_value = "prefixes.txt")]
    pub prefix_pool_file: String,

    /// ASN pool start (inclusive)
    #[arg(long = "asn-pool-start", default_value = "65000")]
    pub asn_pool_start: i32,

    /// ASN pool end (inclusive)
    #[arg(long = "asn-pool-end", default_value = "65999")]
    pub asn_pool_end: i32,

    /// LogTo JWKS URI for JWT validation
    #[arg(long = "logto-jwks-uri")]
    pub logto_jwks_uri: Option<String>,

    /// LogTo issuer for JWT validation
    #[arg(long = "logto-issuer")]
    pub logto_issuer: Option<String>,

    /// Bypass JWT validation (for development only)
    #[arg(long = "bypass-jwt", default_value = "false")]
    pub bypass_jwt: bool,

    /// Agent key for agent authentication
    #[arg(long = "agent-key", default_value = "agent-key")]
    pub agent_key: String,

    /// LogTo Management API URL for fetching user emails
    #[arg(long = "logto-management-api")]
    pub logto_management_api: Option<String>,

    /// LogTo M2M App ID for Management API access
    #[arg(long = "logto-m2m-app-id")]
    pub logto_m2m_app_id: Option<String>,

    /// LogTo M2M App Secret for Management API access
    #[arg(long = "logto-m2m-app-secret")]
    pub logto_m2m_app_secret: Option<String>,

    /// Verbosity level
    #[clap(flatten)]
    verbose: Verbosity<InfoLevel>,
}

fn set_tracing(cli: &Cli) -> Result<()> {
    let subscriber = tracing_subscriber::fmt()
        .compact()
        .with_file(true)
        .with_line_number(true)
        .with_max_level(cli.verbose)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse command line arguments
    let cli = Cli::parse();

    set_tracing(&cli)?;

    // Initialize agent store
    let agent_store = AgentStore::new();

    // Log JWT configuration from CLI parameters
    if let Some(ref jwks_uri) = cli.logto_jwks_uri {
        info!("LogTo JWKS URI is set to: {}", jwks_uri);
    } else {
        warn!("LogTo JWKS URI is not set");
    }

    if let Some(ref issuer) = cli.logto_issuer {
        info!("LogTo issuer is set to: {}", issuer);
    } else {
        warn!("LogTo issuer is not set");
    }

    // Log Logto Management API configuration
    if cli.logto_management_api.is_some()
        && cli.logto_m2m_app_id.is_some()
        && cli.logto_m2m_app_secret.is_some()
    {
        info!("LogTo Management API is configured for email retrieval");
    } else {
        warn!("LogTo Management API is not fully configured - email retrieval will be disabled");
    }

    // Create ASN pool
    let asn_pool = AsnPool::new(cli.asn_pool_start, cli.asn_pool_end);

    // Load prefix pool from file
    let prefix_pool = match PrefixPool::from_file(&cli.prefix_pool_file) {
        Ok(pool) => {
            info!(
                "Loaded prefix pool with {} prefixes from {}",
                pool.len(),
                cli.prefix_pool_file
            );
            pool
        }
        Err(err) => {
            error!(
                "Failed to load prefix pool from {}: {}",
                cli.prefix_pool_file, err
            );
            return Err(anyhow::anyhow!(
                "Failed to load prefix pool from {}: {}",
                cli.prefix_pool_file,
                err
            ));
        }
    };

    // Initialize database
    let database_config = DatabaseConfig::new(cli.database_url.clone());
    let database = match Database::new(&database_config).await {
        Ok(db) => {
            info!("Connected to database: {}", cli.database_url);

            // Run database migrations automatically
            info!("Running database migrations...");
            if let Err(err) = db.initialize().await {
                error!("Failed to run database migrations: {}", err);
                return Err(anyhow::anyhow!(
                    "Failed to run database migrations: {}",
                    err
                ));
            }
            info!("Database migrations completed successfully");
            db
        }
        Err(err) => {
            error!("Failed to connect to database: {}", err);
            return Err(anyhow::anyhow!("Failed to connect to database: {}", err));
        }
    };

    // Create app state
    let state = AppState {
        agent_store,
        agent_key: cli.agent_key.clone(),
        database,
        asn_pool,
        prefix_pool,
        logto_jwks_uri: cli.logto_jwks_uri.clone(),
        logto_issuer: cli.logto_issuer.clone(),
        logto_management_api: cli.logto_management_api.clone(),
        logto_m2m_app_id: cli.logto_m2m_app_id.clone(),
        logto_m2m_app_secret: cli.logto_m2m_app_secret.clone(),
        bypass_jwt_validation: cli.bypass_jwt,
    };

    if cli.bypass_jwt {
        warn!("⚠️ JWT validation bypass is enabled!");
    }

    let app = create_app(state);

    let addr: SocketAddr = cli.address.parse()?;
    info!("Starting server on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
