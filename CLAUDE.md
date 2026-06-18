# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

`peerlab-gateway` is the control-plane HTTP service for nxthdr's PeerLab peering platform. It is an async Rust (tokio) service built with `axum` that assigns private ASNs and time-limited IPv6 /48 prefix leases to authenticated users, persists them in PostgreSQL, manages RPKI ROAs for leased prefixes, and exposes the resulting user→ASN→prefix mappings to downstream BGP/BIRD config generators. It compiles to the `peerlab-gateway` binary.

## Commands

```bash
cargo check --locked              # what CI runs first
cargo test --locked --verbose     # run tests (CI command)
cargo test <name>                 # run a single test by name substring
cargo run -- --bypass-jwt --database-url postgresql://postgres:postgres@localhost/peerlab_gateway --prefix-pool-file prefixes.txt
cargo fmt && cargo clippy         # format + lint (not enforced in CI, but expected)
```

CI (`.github/workflows/cicd.yml`) runs `cargo check`/`cargo test` then builds multi-arch Docker images on every push (apt deps: `libsasl2-dev libssl-dev`). There is no separate lint/fmt gate.

Integration tests need Docker and a real Postgres: `cd integration && docker compose up -d --build --force-recreate --renew-anon-volumes && ./tests/test_gateway.sh && docker compose down` (also `./tests/test_email_retrieval.sh`). The compose stack maps the gateway to host port **8081** (8080 collides with saimiris-gateway).

## Architecture

The whole service is a library crate (`lib.rs`) plus a thin `main.rs` binary; everything hangs off one cloneable `AppState`.

- **`lib.rs`** — all axum handlers, request/response DTOs, and routing. `create_app` mounts two nested routers: `/api/*` (`create_client_app`, JWT-protected) for end users, and `/service/*` (`create_service_app`, agent-key-protected) for downstream services. `AppState` carries the DB pool, both pools, all Auth0/Securebit config, and the `bypass_jwt_validation` flag. `hash_user_identifier` SHA-256s the JWT `sub` into the `user_hash` that is the primary key everywhere — raw user IDs are never stored except `user_id` (kept only for on-demand email lookup).
- **`main.rs`** — clap `Cli` (all config is CLI flags, no env vars), tracing setup, Prometheus metrics exporter on `--metrics-address` (default `:9090`), and startup wiring. Optional subsystems (Securebit, Auth0 Management API) are constructed only when their flags are all present, otherwise they degrade to no-ops with a `warn!`. Runs `sqlx` migrations automatically on boot.
- **`jwt.rs`** — Auth0 JWT validation middleware. Fetches JWKS (RSA + EC keys) and caches the validator in a process-global `once_cell` for 12h. **Audience is NOT verified** (`validate_aud = false`); only issuer + signature + expiry are checked. `--bypass-jwt` injects a fixed dummy `AuthInfo` (`sub = "test-user-id"`) — dev/test only.
- **`database.rs`** — `sqlx` Postgres layer over two tables. Prefixes are stored as `cidr` and always selected back via `prefix::text`. Lease revocation is a **soft delete** (`revoked_at IS NOT NULL`); "active" means `end_time > NOW() AND revoked_at IS NULL`. `cleanup_expired_leases` hard-deletes rows >7 days expired but is not called anywhere yet.
- **`pool_asns.rs`** / **`pool_prefixes.rs`** — allocators. ASN pool is a numeric range (`--asn-pool-start..=end`, default 65000–65999); availability is computed by scanning all DB mappings. Prefix pool is loaded from `--prefix-pool-file` (one `/48` per line, `#` comments; non-/48 lines warned and skipped); both pick the first free entry. Allocation is **not transactional** — concurrent requests can race for the same ASN/prefix.
- **`securebit.rs`** — RPKI ROA automation by **scraping the securebit.cloud web UI** (no API): logs in with form creds, parses the RPKI table with `scraper`, and POSTs the same AJAX `Save()` calls the page's JS would. ROAs are cached in-process for 5 min (`warm_cache` on boot). Securebit is treated as the **source of truth** for `rpki_enabled` — it is queried live, never mirrored in the DB.
- **`auth0.rs`** — fetches a user's email on demand from the Auth0 Management API via an M2M client-credentials token. Email is **never persisted**; it's looked up per request in the `/service` handlers and is `null` if M2M isn't configured.
- **`agent.rs`** — an in-memory `AgentStore` (with caracat probing config / health tracking) that is **declared but not wired into any route**. Treat as dormant/unused unless you add the routes; do not assume it participates in request flow.

## Conventions specific to this repo

- **Config is exclusively CLI flags** parsed by clap in `main.rs` — there is no env-var or config-file layer. Add new config as a `#[arg(long = ...)]` on `Cli`, thread it through `AppState`, and document it in `README.md`.
- **Request/response DTOs live inline in `lib.rs`** next to the handlers (private structs), not in a shared models module. `database.rs` owns the row types (`UserAsnMapping`, `PrefixLease`).
- **Handler errors return `(StatusCode, Json<serde_json::Value>)`** with a `{ "error": <code>, "message": ... }` body and a matching `error!`/`warn!` log line. JWT/agent middleware errors are the exception (`AuthorizationError` with its own `IntoResponse`). Library/infra code uses `anyhow::Result`.
- **Optional integrations fail soft, never hard.** A Securebit or Auth0 failure logs a `warn!` and yields `rpki_enabled: false` / `email: null` rather than failing the request. Preserve this when extending those paths.
- **Increment the relevant `metrics::counter!`** (`peerlab_gateway_asn_assignments_total`, `..._prefix_leases_created_total`, `..._prefix_leases_revoked_total`) when adding success paths to those flows; describe new counters in `set_metrics`.

## Domain notes

- **Ordering invariant:** a user must `POST /api/user/asn` (gets one auto-assigned, sticky ASN) before `POST /api/user/prefix`. Lease duration is clamped to 1–24h; per-user concurrent leases are capped by `user_asn_mappings.max_leases` (DB default 1).
- **RPKI lifecycle:** leasing a prefix adds a ROA for `--securebit-origin-asn` (e.g. AS215011, PeerLab's export ASN); revoking a lease first *ensures the ROA exists* (re-adds it) so RPKI stays valid for in-flight announcements before the lease is released. `PUT .../rpki` toggles the ROA and reconciles against Securebit's live state.
- **No current-state RIB here:** this service only tracks *which user holds which prefix*, not BGP visibility. Downstream services poll `/service/mappings` (Bearer `--agent-key`) to generate BIRD/BGP config; the `user_hash` they receive is the SHA-256 of the Auth0 `sub`.
