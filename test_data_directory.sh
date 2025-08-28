#!/bin/bash

# Test script untuk verifikasi data directory functionality

echo "=== Testing Tabular Data Directory Functionality ==="

# Test 1: Default directory
echo "Test 1: Default data directory"
unset TABULAR_DATA_DIR
rm -f ~/.tabular/config_location.txt 2>/dev/null
echo "Expected: ~/.tabular"
./target/debug/tabular --help 2>&1 | head -1 || echo "App binary not found - need to compile first"

# Test 2: Environment variable
echo -e "\nTest 2: Custom directory via environment variable"
export TABULAR_DATA_DIR="/tmp/tabular_test"
mkdir -p /tmp/tabular_test
echo "Expected: /tmp/tabular_test"
echo "TABULAR_DATA_DIR set to: $TABULAR_DATA_DIR"

# Test 3: Persistent config
echo -e "\nTest 3: Persistent config file"
mkdir -p ~/.tabular
echo "/tmp/tabular_persistent" > ~/.tabular/config_location.txt
mkdir -p /tmp/tabular_persistent
unset TABULAR_DATA_DIR
echo "Expected: /tmp/tabular_persistent (from config file)"
echo "Config file contains: $(cat ~/.tabular/config_location.txt)"

# Cleanup
echo -e "\nCleaning up test directories..."
rm -rf /tmp/tabular_test /tmp/tabular_persistent
rm -f ~/.tabular/config_location.txt

echo "Done. Test by running the application with different configurations."
