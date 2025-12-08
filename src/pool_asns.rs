use tracing::{debug, info};

use crate::database::Database;

/// ASN pool manager
#[derive(Debug, Clone)]
pub struct AsnPool {
    start: i32,
    end: i32,
}

impl AsnPool {
    /// Create a new ASN pool with a range
    pub fn new(start: i32, end: i32) -> Self {
        info!("Created ASN pool: {} - {} ({} ASNs)", start, end, end - start + 1);
        Self { start, end }
    }

    /// Find an available ASN that is not currently assigned in the database
    pub async fn find_available_asn(&self, database: &Database) -> Result<Option<i32>, sqlx::Error> {
        // Get all currently assigned ASNs from database
        let all_mappings = database.get_all_user_mappings().await?;
        let assigned_asns: Vec<i32> = all_mappings.iter().map(|(m, _)| m.asn).collect();

        // Find first available ASN in the pool
        for asn in self.start..=self.end {
            if !assigned_asns.contains(&asn) {
                debug!("Found available ASN: {}", asn);
                return Ok(Some(asn));
            }
        }

        debug!("No available ASNs in pool (all {} ASNs assigned)", self.size());
        Ok(None)
    }

    /// Get the total number of ASNs in the pool
    pub fn size(&self) -> i32 {
        self.end - self.start + 1
    }

    /// Get the start of the ASN range
    pub fn start(&self) -> i32 {
        self.start
    }

    /// Get the end of the ASN range
    pub fn end(&self) -> i32 {
        self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_asn_pool_size() {
        let pool = AsnPool::new(65000, 65999);
        assert_eq!(pool.size(), 1000);
    }

    #[test]
    fn test_asn_pool_range() {
        let pool = AsnPool::new(65000, 65099);
        assert_eq!(pool.start(), 65000);
        assert_eq!(pool.end(), 65099);
        assert_eq!(pool.size(), 100);
    }
}
