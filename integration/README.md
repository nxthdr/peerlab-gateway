# Integration Tests

Tests the Peerlab Gateway with PostgreSQL for ASN assignment and prefix leasing.

## Quick Start

```bash
docker compose up -d --build --force-recreate --renew-anon-volumes
./tests/test_gateway.sh
docker compose down
```

### Test Email Retrieval Only

To specifically test the Logto email retrieval feature:

```bash
docker compose up -d --build --force-recreate --renew-anon-volumes
./tests/test_email_retrieval.sh
docker compose down
```

## What Gets Tested

- Gateway API health
- ASN assignment and persistence
- Prefix leasing (1-24 hours)
- User info endpoint
- Service API authentication (requires agent key)
- Service API for downstream services
- Email retrieval from Logto Management API
- Database persistence

## Manual Testing

```bash
# Get user info
curl http://localhost:8080/api/user/info | jq

# Request ASN (auto-assigned from pool)
curl -X POST http://localhost:8080/api/user/asn | jq

# Request prefix lease
curl -X POST http://localhost:8080/api/user/prefix \
  -H "Content-Type: application/json" \
  -d '{"duration_hours": 1}' | jq

# Get all mappings (service API - requires agent authentication)
curl -H "Authorization: Bearer test-agent-secret-key" \
  http://localhost:8080/service/mappings | jq
```

## Agent Authentication

The service API endpoints (`/service/*`) require agent authentication using a Bearer token:

```bash
# Without authentication (will return 401)
curl http://localhost:8080/service/mappings

# With authentication
curl -H "Authorization: Bearer test-agent-secret-key" \
  http://localhost:8080/service/mappings | jq
```

The response includes user email addresses fetched from Logto:
```json
{
  "mappings": [
    {
      "user_hash": "abc123...",
      "user_id": "auth0-user-id",
      "email": "user@example.com",
      "asn": 65001,
      "prefixes": ["2001:db8:1000::/48"]
    }
  ]
}
```

**Note**: In JWT bypass mode (used for integration tests), the `user_id` is set to `"test-user-id"` which doesn't exist in Logto, so the email will be `null`. To test email retrieval with real data:

1. Disable JWT bypass mode
2. Use a real JWT token from a logged-in user
3. The user's email will be fetched from Logto on-demand

Example with real JWT:
```bash
# Get JWT token from your frontend after login
TOKEN="eyJhbGc..."

# Request ASN with real user
curl -H "Authorization: Bearer $TOKEN" \
  -X POST http://localhost:8080/api/user/asn

# Check service API (requires agent key)
curl -H "Authorization: Bearer test-agent-secret-key" \
  http://localhost:8080/service/mappings | jq
```

## Database Access

```bash
docker compose exec postgres psql -U peerlab_user -d peerlab_gateway
```

```sql
-- View ASN mappings
SELECT * FROM user_asn_mappings;

-- View active leases
SELECT * FROM prefix_leases WHERE end_time > NOW();
```

## Troubleshooting

Check logs:
```bash
docker compose logs gateway
```

Rebuild:
```bash
docker compose build --no-cache gateway
docker compose up -d --force-recreate
```

Reset database:
```bash
docker compose down -v
docker compose up -d
```

## Environment

- **PostgreSQL 17** at 10.0.0.50
- **Gateway** at 10.0.0.10:8080
- **Test prefixes**: 2001:db8:1000::/48 - 1009::/48
- **JWT bypass mode** enabled (dev only)
- **Agent key**: `test-agent-secret-key`
- **Logto Management API**: Not configured by default (email will be `null`)
  - To test email retrieval, add M2M credentials to compose.yml:
    ```yaml
    --auth0-management-api=https://3qo5br.auth0.app
    --auth0-m2m-app-id=<your-app-id>
    --auth0-m2m-app-secret=<your-app-secret>
    ```
