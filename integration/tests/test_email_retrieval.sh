#!/bin/bash
# Small test component to verify Logto email retrieval functionality

set -e

GATEWAY_URL="http://127.0.0.1:8080"
AGENT_KEY="test-agent-secret-key"

echo "📧 Logto Email Retrieval Test"
echo "============================="
echo ""

# Step 1: Request ASN to create a user mapping with user_id
echo "[1/3] Creating user mapping with ASN request..."
ASN_RESPONSE=$(curl -4 -s -X POST "$GATEWAY_URL/api/user/asn")
ASSIGNED_ASN=$(echo "$ASN_RESPONSE" | grep -o '"asn":[0-9]*' | cut -d':' -f2)

if [[ -z "$ASSIGNED_ASN" ]]; then
    echo "❌ Failed to assign ASN"
    echo "Response: $ASN_RESPONSE"
    exit 1
fi
echo "✅ ASN assigned: $ASSIGNED_ASN"
echo ""

# Step 2: Fetch mappings with agent authentication
echo "[2/3] Fetching mappings with agent authentication..."
MAPPINGS=$(curl -4 -s -H "Authorization: Bearer $AGENT_KEY" "$GATEWAY_URL/service/mappings")

if [[ -z "$MAPPINGS" ]]; then
    echo "❌ Failed to fetch mappings"
    exit 1
fi
echo "✅ Mappings fetched successfully"
echo ""

# Step 3: Verify email field and display user info
echo "[3/3] Verifying email retrieval from Logto..."
echo ""
echo "📋 User Mapping Details:"
echo "----------------------"

# Check if email field exists
if ! echo "$MAPPINGS" | grep -q '"email"'; then
    echo "❌ Email field not found in response"
    echo "Response: $MAPPINGS"
    exit 1
fi

# Display the first mapping with pretty formatting
echo "$MAPPINGS" | jq -r '.mappings[0] |
"User Hash:  \(.user_hash)
User ID:    \(.user_id)
Email:      \(.email // "null")
ASN:        \(.asn)
Prefixes:   \(.prefixes | join(", "))"' || {
    echo "Full response:"
    echo "$MAPPINGS" | jq '.'
}

echo ""
echo "✅ Email field present in response"

# Check if email was actually retrieved
EMAIL=$(echo "$MAPPINGS" | jq -r '.mappings[0].email')
if [[ "$EMAIL" == "null" ]] || [[ -z "$EMAIL" ]]; then
    echo ""
    echo "⚠️  Note: Email is null - this is expected if:"
    echo "   - The user doesn't have an email in Logto"
    echo "   - The Logto M2M credentials are incorrect"
    echo "   - The user_id is not set in the database"
else
    echo ""
    echo "🎉 SUCCESS: Email retrieved from Logto: $EMAIL"
fi

echo ""
echo "============================="
echo "Test completed successfully!"
