use chrono::{DateTime, Utc};
use ipnet::Ipv6Net;
use sqlx::PgPool;
use tracing::debug;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub database_url: String,
}

impl DatabaseConfig {
    pub fn new(database_url: String) -> Self {
        Self { database_url }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserAsnMapping {
    pub id: Uuid,
    pub user_hash: String,
    pub asn: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PrefixLease {
    pub id: Uuid,
    pub user_hash: String,
    pub prefix: String,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct Database {
    pool: PgPool,
}

impl Database {
    pub async fn new(config: &DatabaseConfig) -> Result<Self, sqlx::Error> {
        let pool = PgPool::connect(&config.database_url).await?;
        Ok(Self { pool })
    }

    /// Initialize the database by running migrations
    pub async fn initialize(&self) -> Result<(), sqlx::Error> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }

    /// Get or create ASN for a user
    pub async fn get_or_create_user_asn(
        &self,
        user_hash: &str,
        asn: i32,
    ) -> Result<UserAsnMapping, sqlx::Error> {
        // First try to get existing mapping
        let existing = sqlx::query_as::<_, UserAsnMapping>(
            "SELECT * FROM user_asn_mappings WHERE user_hash = $1",
        )
        .bind(user_hash)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(mapping) = existing {
            return Ok(mapping);
        }

        // Create new mapping
        let mapping = sqlx::query_as::<_, UserAsnMapping>(
            "INSERT INTO user_asn_mappings (user_hash, asn) VALUES ($1, $2)
             ON CONFLICT (user_hash) DO UPDATE SET updated_at = NOW()
             RETURNING *",
        )
        .bind(user_hash)
        .bind(asn)
        .fetch_one(&self.pool)
        .await?;

        debug!("Created ASN mapping for user {}: ASN {}", user_hash, asn);
        Ok(mapping)
    }

    /// Get user ASN mapping
    pub async fn get_user_asn(
        &self,
        user_hash: &str,
    ) -> Result<Option<UserAsnMapping>, sqlx::Error> {
        let mapping = sqlx::query_as::<_, UserAsnMapping>(
            "SELECT * FROM user_asn_mappings WHERE user_hash = $1",
        )
        .bind(user_hash)
        .fetch_optional(&self.pool)
        .await?;

        Ok(mapping)
    }

    /// Check if an ASN is already assigned
    pub async fn is_asn_assigned(&self, asn: i32) -> Result<bool, sqlx::Error> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM user_asn_mappings WHERE asn = $1")
                .bind(asn)
                .fetch_one(&self.pool)
                .await?;

        Ok(count > 0)
    }

    /// Create a new prefix lease
    pub async fn create_prefix_lease(
        &self,
        user_hash: &str,
        prefix: &Ipv6Net,
        duration_hours: i32,
    ) -> Result<PrefixLease, sqlx::Error> {
        let start_time = Utc::now();
        let end_time = start_time + chrono::Duration::hours(duration_hours as i64);

        let lease = sqlx::query_as::<_, PrefixLease>(
            "INSERT INTO prefix_leases (user_hash, prefix, start_time, end_time)
             VALUES ($1, $2::cidr, $3, $4)
             RETURNING id, user_hash, prefix::text, start_time, end_time, created_at, updated_at",
        )
        .bind(user_hash)
        .bind(prefix.to_string())
        .bind(start_time)
        .bind(end_time)
        .fetch_one(&self.pool)
        .await?;

        debug!(
            "Created prefix lease for user {}: {} until {}",
            user_hash, prefix, end_time
        );
        Ok(lease)
    }

    /// Get active prefix leases for a user
    pub async fn get_active_user_leases(
        &self,
        user_hash: &str,
    ) -> Result<Vec<PrefixLease>, sqlx::Error> {
        let leases = sqlx::query_as::<_, PrefixLease>(
            "SELECT id, user_hash, prefix::text, start_time, end_time, created_at, updated_at
             FROM prefix_leases
             WHERE user_hash = $1 AND end_time > NOW()
             ORDER BY end_time DESC",
        )
        .bind(user_hash)
        .fetch_all(&self.pool)
        .await?;

        Ok(leases)
    }

    /// Get all active leases (for downstream services)
    pub async fn get_all_active_leases(&self) -> Result<Vec<PrefixLease>, sqlx::Error> {
        let leases = sqlx::query_as::<_, PrefixLease>(
            "SELECT id, user_hash, prefix::text, start_time, end_time, created_at, updated_at
             FROM prefix_leases
             WHERE end_time > NOW()
             ORDER BY end_time DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(leases)
    }

    /// Check if a prefix is currently leased
    pub async fn is_prefix_leased(&self, prefix: &Ipv6Net) -> Result<bool, sqlx::Error> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM prefix_leases
             WHERE prefix = $1::cidr AND end_time > NOW()",
        )
        .bind(prefix.to_string())
        .fetch_one(&self.pool)
        .await?;

        Ok(count > 0)
    }

    /// Clean up expired leases (optional maintenance task)
    pub async fn cleanup_expired_leases(&self) -> Result<u64, sqlx::Error> {
        let result =
            sqlx::query("DELETE FROM prefix_leases WHERE end_time < NOW() - INTERVAL '7 days'")
                .execute(&self.pool)
                .await?;

        Ok(result.rows_affected())
    }

    /// Get user information with ASN and active leases
    pub async fn get_user_info(
        &self,
        user_hash: &str,
    ) -> Result<Option<(Option<UserAsnMapping>, Vec<PrefixLease>)>, sqlx::Error> {
        let asn_mapping = self.get_user_asn(user_hash).await?;
        let leases = self.get_active_user_leases(user_hash).await?;

        Ok(Some((asn_mapping, leases)))
    }

    /// Get all user mappings with their ASN and active leases (for downstream services)
    pub async fn get_all_user_mappings(
        &self,
    ) -> Result<Vec<(UserAsnMapping, Vec<PrefixLease>)>, sqlx::Error> {
        // Get all ASN mappings
        let mappings = sqlx::query_as::<_, UserAsnMapping>(
            "SELECT * FROM user_asn_mappings ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut result = Vec::new();
        for mapping in mappings {
            let leases = self.get_active_user_leases(&mapping.user_hash).await?;
            result.push((mapping, leases));
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn test_database_operations() {
        // This is a placeholder for integration tests
        // In practice, you would use a test database
    }
}
