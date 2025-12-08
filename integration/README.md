# Integration Tests

Tests the Peerlab Gateway with PostgreSQL for ASN assignment and prefix leasing.

## Quick Start

```bash
docker compose up -d --force-recreate --renew-anon-volumes
./tests/test_gateway.sh
docker compose down
```

## What Gets Tested

- Gateway API health
- ASN assignment and persistence
- Prefix leasing (1-24 hours)
- User info endpoint
- Service API for downstream services
- Database persistence

## Manual Testing

```bash
# Get user info
curl http://localhost:8080/api/user/info | jq

# Request ASN
curl -X POST http://localhost:8080/api/user/asn \
  -H "Content-Type: application/json" \
  -d '{"asn": 65001}' | jq

# Request prefix lease
curl -X POST http://localhost:8080/api/user/prefix \
  -H "Content-Type: application/json" \
  -d '{"duration_hours": 1}' | jq

# Get all mappings (service API)
curl http://localhost:8080/service/mappings | jq
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
