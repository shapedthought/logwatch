#!/bin/bash

# Script to generate test logs for LogWatch

set -e

# Create test log directory
TEST_DIR="./test_logs"
mkdir -p "$TEST_DIR/app"
mkdir -p "$TEST_DIR/api"
mkdir -p "$TEST_DIR/database"

echo "Creating test log directories in $TEST_DIR"

# Function to generate random log lines
generate_logs() {
    local file=$1
    local prefix=$2
    
    while true; do
        # Generate random log level
        LEVEL=$((RANDOM % 4))
        case $LEVEL in
            0) LEVEL="DEBUG" ;;
            1) LEVEL="INFO" ;;
            2) LEVEL="WARN" ;;
            3) LEVEL="ERROR" ;;
        esac
        
        # Generate random messages
        MESSAGES=(
            "Database connection successful"
            "Request processed in 150ms"
            "Cache miss for key user_123"
            "Authentication successful"
            "Connection timeout after 30s"
            "Invalid input detected"
            "Retrying failed operation"
            "Health check passed"
            "Configuration reloaded"
            "Memory usage at 75%"
        )
        
        MSG=${MESSAGES[$((RANDOM % ${#MESSAGES[@]}))]}
        
        # Write log line
        echo "$(date '+%Y-%m-%d %H:%M:%S') [$LEVEL] $prefix: $MSG" >> "$file"
        
        # Random delay between 1-5 seconds
        sleep $((RANDOM % 5 + 1))
    done
}

echo "Starting log generation..."
echo "Press Ctrl+C to stop"
echo ""
echo "You can now run logwatch in another terminal:"
echo "  cargo run -- -d $TEST_DIR"
echo "  cargo run -- -d $TEST_DIR -i ERROR -i WARN"
echo ""

# Generate logs in parallel
generate_logs "$TEST_DIR/app/server.log" "Server" &
PID1=$!

generate_logs "$TEST_DIR/api/requests.log" "API" &
PID2=$!

generate_logs "$TEST_DIR/database/queries.log" "DB" &
PID3=$!

# Wait for user to stop
trap "kill $PID1 $PID2 $PID3 2>/dev/null; exit" INT TERM

wait
