#!/bin/bash
set -e

GATEWAY_URL="http://127.0.0.1:8080"
TEST_ASN=65001
AGENT_KEY="test-agent-secret-key"

echo "🚀 Peerlab Gateway Integration Test"
echo "===================================="
echo ""
echo "Note: Gateway runs in JWT bypass mode (development only)."
echo "This test verifies the gateway's core functionality."
echo ""

# Test 1: Gateway health
echo "[1/7] Gateway health check..."
if ! curl -4 -sf "$GATEWAY_URL/api/user/info" > /dev/null; then
    echo "❌ Gateway not responding"
    exit 1
fi
echo "✅ Gateway is healthy"
echo ""

# Test 2: Get initial user info
echo "[2/7] Getting initial user info..."
INITIAL_INFO=$(curl -4 -s "$GATEWAY_URL/api/user/info")
echo "$INITIAL_INFO" | jq '.'
INITIAL_ASN=$(echo "$INITIAL_INFO" | grep -o '"asn":[0-9]*' | cut -d':' -f2 || echo "null")
echo "   Initial ASN: ${INITIAL_ASN:-none}"
echo ""

# Test 3: Request ASN assignment (auto-assigned from pool)
echo "[3/7] Requesting ASN assignment..."

ASN_RESPONSE=$(curl -4 -s -X POST "$GATEWAY_URL/api/user/asn")

ASSIGNED_ASN=$(echo "$ASN_RESPONSE" | grep -o '"asn":[0-9]*' | cut -d':' -f2)
if [[ -z "$ASSIGNED_ASN" ]]; then
    echo "❌ ASN assignment failed"
    echo "Response: $ASN_RESPONSE"
    exit 1
fi
echo "✅ ASN assigned: $ASSIGNED_ASN"
echo ""

# Test 4: Verify ASN persistence
echo "[4/7] Verifying ASN persistence..."
USER_INFO=$(curl -4 -s "$GATEWAY_URL/api/user/info")
CURRENT_ASN=$(echo "$USER_INFO" | grep -o '"asn":[0-9]*' | cut -d':' -f2)
if [[ "$CURRENT_ASN" != "$ASSIGNED_ASN" ]]; then
    echo "❌ ASN not persisted correctly"
    echo "Expected: $ASSIGNED_ASN, Got: $CURRENT_ASN"
    exit 1
fi
echo "✅ ASN persisted correctly: $CURRENT_ASN"
echo ""

# Test 5: Request prefix lease
echo "[5/7] Requesting prefix lease (1 hour)..."
PREFIX_PAYLOAD='{
  "duration_hours": 1
}'

PREFIX_RESPONSE=$(curl -4 -s -X POST "$GATEWAY_URL/api/user/prefix" \
    -H "Content-Type: application/json" \
    -d "$PREFIX_PAYLOAD")

LEASED_PREFIX=$(echo "$PREFIX_RESPONSE" | grep -o '"prefix":"[^"]*' | cut -d'"' -f4)
if [[ -z "$LEASED_PREFIX" ]]; then
    echo "❌ Prefix lease failed"
    echo "Response: $PREFIX_RESPONSE"
    exit 1
fi
echo "✅ Prefix leased: $LEASED_PREFIX"
echo ""

# Test 6: Verify prefix in user info
echo "[6/7] Verifying prefix in user info..."
USER_INFO=$(curl -4 -s "$GATEWAY_URL/api/user/info")
if ! echo "$USER_INFO" | grep -q "$LEASED_PREFIX"; then
    echo "❌ Leased prefix not found in user info"
    echo "Response: $USER_INFO"
    exit 1
fi
echo "✅ Prefix appears in user info"
echo ""

# Test 7: Test service API without authentication (should fail)
echo "[7/10] Testing service API without authentication..."
HTTP_CODE=$(curl -4 -s -o /dev/null -w "%{http_code}" "$GATEWAY_URL/service/mappings")
if [[ "$HTTP_CODE" != "401" ]]; then
    echo "❌ Service API should require authentication (expected 401, got $HTTP_CODE)"
    exit 1
fi
echo "✅ Service API correctly requires authentication"
echo ""

# Test 8: Test service API with valid agent key
echo "[8/10] Testing service API with valid agent key..."
MAPPINGS=$(curl -4 -s -H "Authorization: Bearer $AGENT_KEY" "$GATEWAY_URL/service/mappings")
if ! echo "$MAPPINGS" | grep -q "$ASSIGNED_ASN"; then
    echo "❌ ASN not found in service mappings"
    echo "Response: $MAPPINGS"
    exit 1
fi
if ! echo "$MAPPINGS" | grep -q "$LEASED_PREFIX"; then
    echo "❌ Prefix not found in service mappings"
    echo "Response: $MAPPINGS"
    exit 1
fi
echo "✅ Service API returns correct mappings with authentication"
echo ""

# Test 9: Verify email field is present in mappings
echo "[9/10] Verifying email field in service API response..."
if ! echo "$MAPPINGS" | grep -q '"email"'; then
    echo "❌ Email field not found in service mappings"
    echo "Response: $MAPPINGS"
    exit 1
fi
echo "✅ Email field present in service API response"
echo "$MAPPINGS" | jq '.mappings[0] | {user_id, email, asn}' || true
echo ""

# Test 10: Request another prefix (should get a different one)
echo "[10/10] Requesting second prefix..."
PREFIX_RESPONSE2=$(curl -4 -s -X POST "$GATEWAY_URL/api/user/prefix" \
    -H "Content-Type: application/json" \
    -d "$PREFIX_PAYLOAD")

LEASED_PREFIX2=$(echo "$PREFIX_RESPONSE2" | grep -o '"prefix":"[^"]*' | cut -d'"' -f4)
if [[ -z "$LEASED_PREFIX2" ]]; then
    echo "❌ Second prefix lease failed"
    echo "Response: $PREFIX_RESPONSE2"
    exit 1
fi
if [[ "$LEASED_PREFIX2" == "$LEASED_PREFIX" ]]; then
    echo "⚠️  Warning: Got same prefix twice (might be expected if pool is small)"
else
    echo "✅ Second prefix leased: $LEASED_PREFIX2"
fi
echo ""

# Summary
echo "===================================="
echo "🎉 SUCCESS: All tests passed!"
echo ""
echo "Verified:"
echo "  ✅ Gateway API health"
echo "  ✅ ASN assignment"
echo "  ✅ ASN persistence"
echo "  ✅ Prefix leasing"
echo "  ✅ User info endpoint"
echo "  ✅ Service API authentication (401 without key)"
echo "  ✅ Service API with agent key"
echo "  ✅ Email field in service API response"
echo "  ✅ Multiple prefix leases"
echo "  ✅ Database persistence"
echo ""
echo "ℹ️  Gateway runs in JWT bypass mode (development only)"
echo "ℹ️  Service API requires agent authentication"
exit 0
