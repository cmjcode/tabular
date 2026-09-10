#!/usr/bin/env bash
# ==============================================================================
# Tabular Sync & Authentication API Test Script
# ==============================================================================
# Tests HTTP endpoints for server health, authentication, ticket polling,
# token refresh, and user profile management.
#
# Usage:
#   bash test_api.sh
#   API_URL=http://localhost:8080 ./test_api.sh
# ==============================================================================

set -euo pipefail

API_URL="${API_URL:-https://api.tabular.id}"
PASSED_TESTS=0
FAILED_TESTS=0

CLIENT_VERSION="0.15.0"

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}======================================================${NC}"
echo -e "${BLUE}    Tabular API & Account Sync Integration Tests     ${NC}"
echo -e "${BLUE}======================================================${NC}"
echo -e "Target Server: ${YELLOW}${API_URL}${NC}\n"

# Helper function to assert HTTP status code
# Arguments:
#   $1: Test Name
#   $2: Expected status codes (regex, e.g. "200|400")
#   $3: Actual status code
#   $4: Response body
assert_status() {
    local test_name="$1"
    local expected="$2"
    local actual="$3"
    local body="$4"

    if [[ "$actual" =~ ^($expected)$ ]]; then
        echo -e "  [${GREEN}PASS${NC}] ${test_name} (HTTP ${actual})"
        PASSED_TESTS=$((PASSED_TESTS + 1))
    else
        echo -e "  [${RED}FAIL${NC}] ${test_name} (Expected: ${expected}, Got: ${actual})"
        if [ -n "$body" ]; then
            echo -e "         Response: ${body:0:160}"
        fi
        FAILED_TESTS=$((FAILED_TESTS + 1))
    fi
}

# 1. Health Check
echo -e "${YELLOW}[1/5] Checking Server Health...${NC}"
HEALTH_RESP=$(curl -s -w "\n%{http_code}" -X GET "${API_URL}/health" \
    -H "X-Tabular-Client-Version: ${CLIENT_VERSION}" || true)
HEALTH_BODY=$(echo "$HEALTH_RESP" | head -n -1)
HEALTH_CODE=$(echo "$HEALTH_RESP" | tail -n 1)
assert_status "GET /health" "200" "$HEALTH_CODE" "$HEALTH_BODY"

# 2. OAuth Session Ticket Polling Endpoint
echo -e "\n${YELLOW}[2/5] Testing OAuth Ticket Poll Endpoint...${NC}"
POLL_PAYLOAD='{"ticket":"0123456789abcdef0123456789abcdef"}'
POLL_RESP=$(curl -s -w "\n%{http_code}" -X POST "${API_URL}/api/v1/auth/ticket/poll" \
    -H "Content-Type: application/json" \
    -H "X-Tabular-Client-Version: ${CLIENT_VERSION}" \
    -d "$POLL_PAYLOAD" || true)
POLL_BODY=$(echo "$POLL_RESP" | head -n -1)
POLL_CODE=$(echo "$POLL_RESP" | tail -n 1)
assert_status "POST /api/v1/auth/ticket/poll" "200|202|400" "$POLL_CODE" "$POLL_BODY"

# 3. Token Refresh Endpoint
echo -e "\n${YELLOW}[3/5] Testing Token Refresh Endpoint...${NC}"
REFRESH_PAYLOAD='{"refresh_token":"test_dummy_token"}'
REFRESH_RESP=$(curl -s -w "\n%{http_code}" -X POST "${API_URL}/api/v1/auth/refresh" \
    -H "Content-Type: application/json" \
    -H "X-Tabular-Client-Version: ${CLIENT_VERSION}" \
    -d "$REFRESH_PAYLOAD" || true)
REFRESH_BODY=$(echo "$REFRESH_RESP" | head -n -1)
REFRESH_CODE=$(echo "$REFRESH_RESP" | tail -n 1)
assert_status "POST /api/v1/auth/refresh" "400|401" "$REFRESH_CODE" "$REFRESH_BODY"

# 4. User Profile Update Endpoint (PUT /api/v1/users/me)
echo -e "\n${YELLOW}[4/5] Testing Profile Update Endpoint...${NC}"
PROFILE_PAYLOAD='{"display_name":"Test User","username":"testuser","phone":"+6281234567890"}'
PROFILE_RESP=$(curl -s -w "\n%{http_code}" -X PUT "${API_URL}/api/v1/users/me" \
    -H "Content-Type: application/json" \
    -H "X-Tabular-Client-Version: ${CLIENT_VERSION}" \
    -H "Authorization: Bearer mock_or_expired_token" \
    -d "$PROFILE_PAYLOAD" || true)
PROFILE_BODY=$(echo "$PROFILE_RESP" | head -n -1)
PROFILE_CODE=$(echo "$PROFILE_RESP" | tail -n 1)
assert_status "PUT /api/v1/users/me" "401|403" "$PROFILE_CODE" "$PROFILE_BODY"

# 5. User Search Endpoint (GET /api/v1/users/search)
echo -e "\n${YELLOW}[5/5] Testing User Search Endpoint...${NC}"
SEARCH_RESP=$(curl -s -w "\n%{http_code}" -X GET "${API_URL}/api/v1/users/search?q=testuser" \
    -H "X-Tabular-Client-Version: ${CLIENT_VERSION}" \
    -H "Authorization: Bearer mock_or_expired_token" || true)
SEARCH_BODY=$(echo "$SEARCH_RESP" | head -n -1)
SEARCH_CODE=$(echo "$SEARCH_RESP" | tail -n 1)
assert_status "GET /api/v1/users/search" "200|401|403" "$SEARCH_CODE" "$SEARCH_BODY"

# Summary
echo -e "\n${BLUE}======================================================${NC}"
echo -e "Test Summary: ${GREEN}${PASSED_TESTS} Passed${NC}, ${RED}${FAILED_TESTS} Failed${NC}"
echo -e "${BLUE}======================================================${NC}"

if [ "$FAILED_TESTS" -eq 0 ]; then
    echo -e "${GREEN}All API endpoint tests succeeded!${NC}\n"
    exit 0
else
    echo -e "${RED}Some API tests failed.${NC}\n"
    exit 1
fi
