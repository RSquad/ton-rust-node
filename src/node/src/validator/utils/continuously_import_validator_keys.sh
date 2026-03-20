#!/bin/bash

# continuously_import_validator_keys.sh - Continuously import validator keys
# This script runs import_validator_keys.sh once per minute to automatically
# import new validator keys from the C++ node configuration

# Get script directory and import script path (needed for all commands)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMPORT_SCRIPT_PATH="$SCRIPT_DIR/import_validator_keys.sh"

# Display usage information
function show_usage {
    echo "Usage: $0 <cpp_node_path> <console_config_path> [sleep_interval] [log_directory]"
    echo "       $0 {start|stop|status|restart} <cpp_node_path> <console_config_path> [sleep_interval] [log_directory]"
    echo ""
    echo "Parameters:"
    echo "  cpp_node_path      - Path to C++ node directory"
    echo "  console_config_path - Path to console configuration file"
    echo "  sleep_interval     - Sleep time in seconds (default: 60)"
    echo "  log_directory      - Directory for log and PID files (default: script directory)"
    echo ""
    echo "Daemon commands:"
    echo "  start              - Start the daemon (default if no command given)"
    echo "  stop               - Stop the running daemon"
    echo "  status             - Show daemon status"
    echo "  restart            - Restart the daemon"
    echo ""
    echo "Example:"
    echo "  $0 /path/to/cpp/node /path/to/console.json"
    echo "  $0 start /path/to/cpp/node /path/to/console.json 30 /path/to/logs"
    echo "  $0 stop /path/to/cpp/node /path/to/console.json 60 /path/to/logs"
    echo "  $0 status /path/to/cpp/node /path/to/console.json 60 /path/to/logs"
    echo ""
    echo "The script runs automatically in the background as a daemon"
    exit 1
}

# Function to set up paths based on arguments
setup_paths() {
    local cpp_path="$1"
    local console_path="$2" 
    local sleep_int="${3:-60}"
    local log_dir="${4:-}"
    
    # Set log directory - use provided directory or default to script directory
    if [ -n "$log_dir" ]; then
        LOG_DIR="$log_dir"
    else
        LOG_DIR="$SCRIPT_DIR"
    fi
    
    # Set file paths
    LOG_FILE="$LOG_DIR/validator_import_monitor.log"
    PID_FILE="$LOG_DIR/validator_import_monitor.pid"
}

# Function to check if daemon is running
is_daemon_running() {
    if [ -f "$PID_FILE" ]; then
        local pid=$(cat "$PID_FILE")
        if kill -0 "$pid" 2>/dev/null; then
            return 0  # Running
        else
            rm -f "$PID_FILE"  # Clean up stale PID file
            return 1  # Not running
        fi
    fi
    return 1  # Not running
}

# Function to start daemon
start_daemon() {
    if is_daemon_running; then
        echo "Daemon is already running (PID: $(cat "$PID_FILE"))"
        return 1
    fi
    
    echo "Starting validator import monitor daemon..."
    echo "Logs will be written to: $LOG_FILE"
    
    # Start daemon in background
    DAEMON_MODE=1 nohup "$0" "$@" >> "$LOG_FILE" 2>&1 &
    local pid=$!
    echo $pid > "$PID_FILE"
    
    # Wait a moment and check if it started successfully
    sleep 2
    if is_daemon_running; then
        echo "Daemon started successfully (PID: $pid)"
        echo "Use '$0 stop' to stop the daemon"
        echo "Use '$0 status' to check daemon status"
        echo "Monitor logs with: tail -f $LOG_FILE"
        return 0
    else
        echo "Failed to start daemon"
        echo "Check the log file for details: $LOG_FILE"
        if [ -f "$LOG_FILE" ]; then
            echo "Last few log entries:"
            tail -5 "$LOG_FILE" | sed 's/^/  /'
        fi
        return 1
    fi
}

# Function to stop daemon
stop_daemon() {
    if ! is_daemon_running; then
        echo "Daemon is not running"
        return 1
    fi
    
    local pid=$(cat "$PID_FILE")
    echo "Stopping validator import monitor daemon (PID: $pid)..."
    
    if kill "$pid" 2>/dev/null; then
        # Wait for graceful shutdown
        local count=0
        while kill -0 "$pid" 2>/dev/null && [ $count -lt 10 ]; do
            sleep 1
            count=$((count + 1))
        done
        
        if kill -0 "$pid" 2>/dev/null; then
            echo "Force killing daemon..."
            kill -9 "$pid" 2>/dev/null
        fi
        
        rm -f "$PID_FILE"
        echo "Daemon stopped"
        return 0
    else
        echo "Failed to stop daemon"
        rm -f "$PID_FILE"
        return 1
    fi
}

# Function to show daemon status
show_status() {
    if is_daemon_running; then
        local pid=$(cat "$PID_FILE")
        echo "Daemon is running (PID: $pid)"
        echo "Log file: $LOG_FILE"
        if [ -f "$LOG_FILE" ]; then
            echo "Last log entries:"
            tail -5 "$LOG_FILE" | sed 's/^/  /'
        fi
        return 0
    else
        echo "Daemon is not running"
        return 1
    fi
}

# Parse command line arguments - handle daemon commands first
case "$1" in
    "start")
        shift  # Remove 'start' from arguments
        setup_paths "$@"
        start_daemon "$@"
        exit $?
        ;;
    "stop")
        shift  # Remove 'stop' from arguments
        setup_paths "$@"
        stop_daemon
        exit $?
        ;;
    "status")
        shift  # Remove 'status' from arguments  
        setup_paths "$@"
        show_status
        exit $?
        ;;
    "restart")
        shift  # Remove 'restart' from arguments
        setup_paths "$@"
        stop_daemon
        sleep 2
        start_daemon "$@"
        exit $?
        ;;
    "-h"|"--help")
        show_usage
        ;;
esac

# If not in daemon mode and no explicit command given, start daemon
if [ -z "$DAEMON_MODE" ]; then
    # Check if we have the required arguments for start
    if [ $# -lt 2 ]; then
        echo "Error: Insufficient arguments for start command"
        show_usage
    fi
    setup_paths "$@"
    start_daemon "$@"
    exit $?
fi

# If we get here, we're running in daemon mode - continue with normal processing
# Parse arguments for daemon execution
if [ $# -lt 2 ]; then
    echo "Error: Insufficient arguments for daemon mode"
    exit 1
fi

CPP_NODE_PATH="$1"
CONSOLE_CONFIG_PATH="$2" 
SLEEP_INTERVAL="${3:-60}"
LOG_DIRECTORY="${4:-}"

# Set up paths for daemon mode
setup_paths "$@"

# Validate paths
echo "Validating paths..."
if [ ! -d "$CPP_NODE_PATH" ]; then
    echo "Error: C++ node path does not exist: $CPP_NODE_PATH"
    exit 1
fi
echo "✓ C++ node path exists: $CPP_NODE_PATH"

if [ ! -f "$CONSOLE_CONFIG_PATH" ]; then
    echo "Error: Console config file does not exist: $CONSOLE_CONFIG_PATH"
    exit 1
fi
echo "✓ Console config exists: $CONSOLE_CONFIG_PATH"

if [ ! -f "$IMPORT_SCRIPT_PATH" ]; then
    echo "Error: Import script does not exist: $IMPORT_SCRIPT_PATH"
    exit 1
fi
echo "✓ Import script exists: $IMPORT_SCRIPT_PATH"

if [ ! -x "$IMPORT_SCRIPT_PATH" ]; then
    echo "Error: Import script is not executable: $IMPORT_SCRIPT_PATH"
    exit 1
fi
echo "✓ Import script is executable"

# Validate sleep interval is a positive number
if ! [[ "$SLEEP_INTERVAL" =~ ^[0-9]+$ ]] || [ "$SLEEP_INTERVAL" -le 0 ]; then
    echo "Error: Sleep interval must be a positive integer (seconds): $SLEEP_INTERVAL"
    exit 1
fi
echo "✓ Sleep interval is valid: ${SLEEP_INTERVAL}s"

# Validate log directory 
if [ ! -d "$LOG_DIR" ]; then
    echo "Error: Log directory does not exist: $LOG_DIR"
    exit 1
fi
if [ ! -w "$LOG_DIR" ]; then
    echo "Error: Log directory is not writable: $LOG_DIR"
    exit 1
fi
echo "✓ Log directory is valid and writable: $LOG_DIR"

# Function to log messages with timestamp
log_message() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] $1"
}

# Function to handle cleanup on exit
cleanup() {
    log_message "Received signal, shutting down gracefully..."
    if [ -n "$DAEMON_MODE" ] && [ -f "$PID_FILE" ]; then
        rm -f "$PID_FILE"
    fi
    exit 0
}

# Set up signal handlers for graceful shutdown
trap cleanup SIGINT SIGTERM

# Display startup information
log_message "Starting continuous validator key import monitor (daemon mode)"
log_message "C++ Node Path: $CPP_NODE_PATH"
log_message "Console Config: $CONSOLE_CONFIG_PATH"
log_message "Import Script: $IMPORT_SCRIPT_PATH"
log_message "Log Directory: $LOG_DIR"
log_message "Sleep Interval: ${SLEEP_INTERVAL}s"
log_message "PID: $$"
echo ""

# Initialize counters
total_runs=0
successful_runs=0
failed_runs=0

# Main continuous loop
while true; do
    total_runs=$((total_runs + 1))
    
    log_message "=== Run #$total_runs - Starting validator key import ==="
    
    # Run the import script and capture its exit code
    if "$IMPORT_SCRIPT_PATH" "$CPP_NODE_PATH" "$CONSOLE_CONFIG_PATH"; then
        successful_runs=$((successful_runs + 1))
        log_message "✓ Run #$total_runs completed successfully"
    else
        failed_runs=$((failed_runs + 1))
        log_message "✗ Run #$total_runs failed with exit code $?"
    fi
    
    # Display statistics
    log_message "Statistics: Total: $total_runs, Successful: $successful_runs, Failed: $failed_runs"
    
    # Sleep before next run
    log_message "Sleeping for ${SLEEP_INTERVAL}s until next run..."
    echo ""
    
    sleep "$SLEEP_INTERVAL"
done 