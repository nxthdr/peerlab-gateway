use anyhow::Result;
use ipnet::Ipv6Net;
use std::fs;
use std::path::Path;
use std::str::FromStr;
use tracing::{debug, info};

/// Prefix pool manager that loads prefixes from a file
#[derive(Debug, Clone)]
pub struct PrefixPool {
    prefixes: Vec<Ipv6Net>,
}

impl PrefixPool {
    /// Load prefixes from a file (one prefix per line)
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = fs::read_to_string(path.as_ref())?;
        let mut prefixes = Vec::new();

        for (line_num, line) in content.lines().enumerate() {
            let line = line.trim();

            // Skip empty lines and comments
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            match Ipv6Net::from_str(line) {
                Ok(prefix) => {
                    // Validate that it's a /48 prefix
                    if prefix.prefix_len() == 48 {
                        prefixes.push(prefix);
                    } else {
                        tracing::warn!(
                            "Line {}: Prefix {} is not a /48, skipping",
                            line_num + 1,
                            line
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Line {}: Failed to parse prefix '{}': {}",
                        line_num + 1,
                        line,
                        e
                    );
                }
            }
        }

        info!("Loaded {} prefixes from file", prefixes.len());
        Ok(Self { prefixes })
    }

    /// Get all available prefixes
    pub fn get_all_prefixes(&self) -> &[Ipv6Net] {
        &self.prefixes
    }

    /// Get the number of prefixes in the pool
    pub fn len(&self) -> usize {
        self.prefixes.len()
    }

    /// Check if the pool is empty
    pub fn is_empty(&self) -> bool {
        self.prefixes.is_empty()
    }

    /// Find an available prefix that is not currently leased
    pub fn find_available_prefix(&self, leased_prefixes: &[Ipv6Net]) -> Option<Ipv6Net> {
        for prefix in &self.prefixes {
            if !leased_prefixes.contains(prefix) {
                debug!("Found available prefix: {}", prefix);
                return Some(*prefix);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_load_prefixes_from_file() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "2001:db8:1::/48").unwrap();
        writeln!(file, "2001:db8:2::/48").unwrap();
        writeln!(file, "# This is a comment").unwrap();
        writeln!(file, "").unwrap();
        writeln!(file, "2001:db8:3::/48").unwrap();

        let pool = PrefixPool::from_file(file.path()).unwrap();
        assert_eq!(pool.len(), 3);
    }

    #[test]
    fn test_find_available_prefix() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "2001:db8:1::/48").unwrap();
        writeln!(file, "2001:db8:2::/48").unwrap();
        writeln!(file, "2001:db8:3::/48").unwrap();

        let pool = PrefixPool::from_file(file.path()).unwrap();

        let leased = vec![Ipv6Net::from_str("2001:db8:1::/48").unwrap()];
        let available = pool.find_available_prefix(&leased);

        assert!(available.is_some());
        assert_ne!(
            available.unwrap(),
            Ipv6Net::from_str("2001:db8:1::/48").unwrap()
        );
    }
}
