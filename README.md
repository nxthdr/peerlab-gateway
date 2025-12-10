# Peerlab Gateway

A Rust-based gateway service for managing IPv6 prefix leases and ASN assignments for the Peerlab project. This service allows authenticated users to request ASN assignments and time-limited IPv6 /48 prefix leases, and provides endpoints for downstream services to query user-to-prefix mappings.

## Features

- **ASN Management**: Users can request and maintain ASN assignments
- **Prefix Leasing**: Time-based IPv6 /48 prefix allocation from a configurable pool
- **JWT Authentication**: Secure API access using LogTo JWT tokens
- **Agent Authentication**: Service API endpoints protected with Bearer token authentication
- **Email Retrieval**: On-demand email fetching from LogTo Management API (no email storage)
- **PostgreSQL Storage**: Persistent storage of user mappings and lease information
- **Service API**: Authenticated endpoints for downstream services to query user mappings

## Architecture

The service provides two main API surfaces:

1. **Client API** (`/api/*`): JWT-authenticated endpoints for end users via nxthdr.dev
2. **Service API** (`/service/*`): Agent-authenticated endpoints for downstream services to query mappings (requires Bearer token)

## API Endpoints

### Client API (JWT Required)

#### `GET /api/user/info`
Get user information including ASN and active prefix leases.

**Response:**
```json
{
  "user_hash": "abc123...",
  "asn": 65001,
  "active_leases": [
    {
      "prefix": "2001:db8:1000::/48",
      "start_time": "2025-01-01T00:00:00Z",
      "end_time": "2025-01-01T01:00:00Z"
    }
  ]
}
```

#### `POST /api/user/asn`
Request an ASN assignment. The gateway automatically assigns an available ASN from the pool. Once assigned, the same ASN is always returned for the user.

**Request:** No body required

**Response:**
```json
{
  "asn": 65001,
  "message": "ASN assigned successfully"
}
```

#### `POST /api/user/prefix`
Request a time-limited IPv6 /48 prefix lease.

**Request:**
```json
{
  "duration_hours": 1
}
```

**Response:**
```json
{
  "prefix": "2001:db8:1000::/48",
  "start_time": "2025-01-01T00:00:00Z",
  "end_time": "2025-01-01T01:00:00Z",
  "message": "Prefix leased successfully"
}
```

### Service API (Agent Authentication Required)

All service API endpoints require agent authentication using a Bearer token in the `Authorization` header.

**Authentication Header:**
```
Authorization: Bearer <agent-key>
```

#### `GET /service/mappings`
Get all user mappings with ASN, active prefixes, and email addresses.

**Response:**
```json
{
  "mappings": [
    {
      "user_hash": "abc123...",
      "user_id": "logto-user-id",
      "email": "user@example.com",
      "asn": 65001,
      "prefixes": ["2001:db8:1000::/48"]
    }
  ]
}
```

**Note:** The `email` field is fetched on-demand from LogTo Management API and is not stored in the database. It will be `null` if LogTo M2M credentials are not configured or if the user doesn't have an email.

#### `GET /service/mappings/:user_hash`
Get mapping for a specific user.

**Response:**
```json
{
  "user_hash": "abc123...",
  "user_id": "logto-user-id",
  "email": "user@example.com",
  "asn": 65001,
  "prefixes": ["2001:db8:1000::/48"]
}
```

## Configuration

### Command Line Arguments

#### Basic Configuration
- `--address`: API listen address (default: `0.0.0.0:8080`)
- `--database-url`: PostgreSQL connection URL (default: `postgresql://localhost/peerlab_gateway`)
- `--prefix-pool-file`: Path to prefix pool file (default: `prefixes.txt`)
- `--asn-pool-start`: ASN pool start (default: `65000`)
- `--asn-pool-end`: ASN pool end (default: `65999`, provides 1000 ASNs)

#### JWT Authentication (Client API)
- `--logto-jwks-uri`: LogTo JWKS URI for JWT validation
- `--logto-issuer`: LogTo issuer for JWT validation
- `--bypass-jwt`: Bypass JWT validation (development only)

#### Agent Authentication (Service API)
- `--agent-key`: Agent key for service API authentication (default: `agent-key`)

#### Email Retrieval (Optional)
- `--logto-management-api`: LogTo Management API URL (e.g., `https://your-instance.logto.app`)
- `--logto-m2m-app-id`: LogTo M2M application ID for Management API access
- `--logto-m2m-app-secret`: LogTo M2M application secret for Management API access

**Note:** Email retrieval is optional. If M2M credentials are not provided, the `email` field in service API responses will be `null`.

### Prefix Pool File

Create a `prefixes.txt` file with one /48 IPv6 prefix per line:

```
2001:db8:1000::/48
2001:db8:1001::/48
2001:db8:1002::/48
```

Lines starting with `#` are treated as comments. See `prefixes.txt.example` for a template.

## Database Schema

The service uses PostgreSQL with two main tables:

### `user_asn_mappings`
Stores the mapping between users and their assigned ASN.

| Column | Type | Description |
|--------|------|-------------|
| id | UUID | Primary key |
| user_hash | VARCHAR(64) | SHA256 hash of user identifier (unique) |
| user_id | VARCHAR(255) | LogTo user ID for email retrieval (nullable) |
| asn | INTEGER | Assigned ASN (unique) |
| created_at | TIMESTAMP | Creation timestamp |
| updated_at | TIMESTAMP | Last update timestamp |

### `prefix_leases`
Stores time-limited prefix leases.

| Column | Type | Description |
|--------|------|-------------|
| id | UUID | Primary key |
| user_hash | VARCHAR(64) | SHA256 hash of user identifier |
| prefix | CIDR | Leased IPv6 prefix |
| start_time | TIMESTAMP | Lease start time |
| end_time | TIMESTAMP | Lease expiration time |
| created_at | TIMESTAMP | Creation timestamp |
| updated_at | TIMESTAMP | Last update timestamp |

## Development

### Prerequisites

- Rust 1.70 or later
- PostgreSQL 14 or later

### Building

```bash
cargo build --release
```

### Running Locally

1. Start PostgreSQL:
```bash
docker run -d --name peerlab-postgres \
  -e POSTGRES_DB=peerlab_gateway \
  -e POSTGRES_PASSWORD=postgres \
  -p 5432:5432 \
  postgres:16
```

2. Create prefix pool file:
```bash
cp prefixes.txt.example prefixes.txt
```

3. Run the service:
```bash
cargo run -- \
  --database-url postgresql://postgres:postgres@localhost/peerlab_gateway \
  --prefix-pool-file prefixes.txt \
  --bypass-jwt
```

### Testing

Run unit tests:
```bash
cargo test
```

Run integration tests (requires Docker):
```bash
cd integration
docker compose up -d --force-recreate --renew-anon-volumes
./tests/test_gateway.sh
docker compose down
```

See [integration/README.md](integration/README.md) for manual testing and troubleshooting.

## Docker

Build the Docker image:

```bash
docker build -t peerlab-gateway .
```

Run the container:

```bash
docker run -d \
  -p 8080:8080 \
  -v $(pwd)/prefixes.txt:/app/prefixes.txt \
  -e DATABASE_URL=postgresql://user:pass@host/db \
  peerlab-gateway \
  --database-url $DATABASE_URL \
  --prefix-pool-file /app/prefixes.txt \
  --logto-jwks-uri https://your-logto.com/.well-known/jwks.json \
  --logto-issuer https://your-logto.com
```

## Integration with nxthdr.dev

The nxthdr.dev frontend should:

1. Authenticate users via LogTo and obtain JWT tokens
2. Call `POST /api/user/asn` to request an ASN (if not already assigned)
3. Call `POST /api/user/prefix` to request prefix leases with desired duration
4. Call `GET /api/user/info` to display current ASN and active leases

## Integration with Downstream Services

Downstream services (e.g., BGP configuration generators, BIRD config generators) must authenticate using an agent key.

### Authentication

All service API requests must include the agent key in the `Authorization` header:

```bash
curl -H "Authorization: Bearer <agent-key>" \
  https://gateway.example.com/service/mappings
```

### Usage

Downstream services can:

1. Call `GET /service/mappings` to get all current user-to-ASN-to-prefix mappings with email addresses
2. Call `GET /service/mappings/:user_hash` to query specific user mappings
3. Poll these endpoints periodically to stay synchronized with active leases

### Response Data

The service API returns:
- `user_hash`: SHA256 hash of the user identifier
- `user_id`: LogTo user ID
- `email`: User's email address (fetched from LogTo on-demand, may be `null`)
- `asn`: Assigned ASN
- `prefixes`: List of active IPv6 /48 prefixes

## License

See LICENSE file for details.
