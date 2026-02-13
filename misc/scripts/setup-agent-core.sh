#!/usr/bin/env bash
#
# Setup script for the agent_core sample.
# Creates AWS AgentCore memory store, S3 bucket, and updates .env
#
# Usage: ./scripts/setup-agent-core.sh [options]
#
# Options:
#   --name NAME       Base name for resources (default: sideseat-demo)
#   --region REGION   AWS region (default: us-east-1)
#   --skip-memory     Skip memory creation
#   --skip-s3         Skip S3 bucket creation
#   --dry-run         Show what would be created without creating
#   -h, --help        Show this help message

set -euo pipefail

# Default configuration
NAME_PREFIX="${NAME_PREFIX:-sideseat-demo}"
AWS_REGION="${AWS_REGION:-us-east-1}"
SKIP_MEMORY=false
SKIP_S3=false
DRY_RUN=false

# Script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SAMPLES_DIR="$(dirname "$SCRIPT_DIR")"
ENV_FILE="$SAMPLES_DIR/.env"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1" >&2; }

usage() {
    grep '^#' "$0" | grep -v '#!/' | cut -c 3-
    exit 0
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --name) NAME_PREFIX="$2"; shift 2 ;;
        --region) AWS_REGION="$2"; shift 2 ;;
        --skip-memory) SKIP_MEMORY=true; shift ;;
        --skip-s3) SKIP_S3=true; shift ;;
        --dry-run) DRY_RUN=true; shift ;;
        -h|--help) usage ;;
        *) log_error "Unknown option: $1"; usage ;;
    esac
done

# Verify AWS credentials
verify_aws_credentials() {
    log_info "Verifying AWS credentials..."
    if ! aws sts get-caller-identity --region "$AWS_REGION" &>/dev/null; then
        log_error "AWS credentials not configured. Run 'aws configure' first."
        exit 1
    fi
    local account_id
    account_id=$(aws sts get-caller-identity --query Account --output text)
    log_info "Using AWS account: $account_id"
}

# Create AgentCore memory using Python/boto3 (AWS CLI doesn't support this)
create_memory() {
    # Memory name: alphanumeric + underscore only, start with letter
    local memory_name="${NAME_PREFIX//-/_}_memory"

    log_info "Creating AgentCore memory: $memory_name"

    if [[ "$DRY_RUN" == "true" ]]; then
        log_info "[DRY-RUN] Would create memory: $memory_name"
        echo "MEMORY_ID=memory_dry-run-id"
        echo "STRATEGY_ID=default_semantic-dry-run-id"
        return 0
    fi

    # Use Python for AgentCore API (not available in AWS CLI)
    uv run python << PYTHON_EOF
import boto3
import sys
import time

region = "$AWS_REGION"
memory_name = "$memory_name"

client = boto3.client('bedrock-agentcore-control', region_name=region)

def wait_for_memory_active(memory_id, max_wait=180):
    """Wait for memory to become active (can take 2-3 minutes)."""
    for _ in range(max_wait // 5):
        details = client.get_memory(memoryId=memory_id)
        status = details['memory']['status']
        if status == 'ACTIVE':
            return details
        if status in ('FAILED', 'DELETING', 'DELETED'):
            raise Exception(f"Memory in bad state: {status}")
        print(f"Waiting for memory to become active (status: {status})...", file=sys.stderr)
        time.sleep(5)
    raise Exception("Timeout waiting for memory to become active")

def get_strategy_id(details):
    """Extract first strategy ID from memory details."""
    strategies = details['memory'].get('strategies', [])
    return strategies[0]['strategyId'] if strategies else ''

# Check if memory with this name already exists (ID starts with name)
try:
    memories = client.list_memories()
    for mem in memories.get('memories', []):
        # Memory ID format: {name}-{random} e.g. sideseat_demo_memory-v0jSZd6SGo
        if mem['id'].startswith(memory_name):
            print(f"Memory already exists: {mem['id']}", file=sys.stderr)
            details = wait_for_memory_active(mem['id'])
            strategy_id = get_strategy_id(details)
            print(f"MEMORY_ID={mem['id']}")
            print(f"STRATEGY_ID={strategy_id}")
            sys.exit(0)
except Exception as e:
    print(f"Warning: Could not list memories: {e}", file=sys.stderr)

# Create new memory with semantic strategy
try:
    response = client.create_memory(
        name=memory_name,
        description="AgentCore memory for SideSeat demo agent",
        eventExpiryDuration=90,  # days
        memoryStrategies=[
            {
                'semanticMemoryStrategy': {
                    'name': 'default_semantic',
                    'description': 'Semantic memory for user preferences and facts',
                    'namespaces': ['default']
                }
            }
        ]
    )
    memory_id = response['memory']['id']
    print(f"Created memory: {memory_id}", file=sys.stderr)

    # Wait for memory to become active and get strategy ID
    details = wait_for_memory_active(memory_id)
    strategy_id = get_strategy_id(details)

    print(f"MEMORY_ID={memory_id}")
    print(f"STRATEGY_ID={strategy_id}")

except Exception as e:
    print(f"Error creating memory: {e}", file=sys.stderr)
    sys.exit(1)
PYTHON_EOF
}

# Create S3 bucket
create_s3_bucket() {
    local bucket_name="${NAME_PREFIX}-sessions-$(aws sts get-caller-identity --query Account --output text)"

    log_info "Creating S3 bucket: $bucket_name"

    if [[ "$DRY_RUN" == "true" ]]; then
        log_info "[DRY-RUN] Would create bucket: $bucket_name"
        echo "BUCKET_NAME=$bucket_name"
        return 0
    fi

    # Check if bucket exists
    if aws s3api head-bucket --bucket "$bucket_name" 2>/dev/null; then
        log_info "Bucket already exists: $bucket_name"
        echo "BUCKET_NAME=$bucket_name"
        return 0
    fi

    # Create bucket (handle us-east-1 specially - no LocationConstraint needed)
    if [[ "$AWS_REGION" == "us-east-1" ]]; then
        aws s3api create-bucket --bucket "$bucket_name" --region "$AWS_REGION"
    else
        aws s3api create-bucket --bucket "$bucket_name" --region "$AWS_REGION" \
            --create-bucket-configuration LocationConstraint="$AWS_REGION"
    fi

    # Enable versioning for safety
    aws s3api put-bucket-versioning --bucket "$bucket_name" \
        --versioning-configuration Status=Enabled

    log_info "Created bucket with versioning: $bucket_name"
    echo "BUCKET_NAME=$bucket_name"
}

# Update .env file
update_env_file() {
    local memory_id="$1"
    local strategy_id="$2"
    local bucket_name="$3"

    log_info "Updating $ENV_FILE"

    if [[ "$DRY_RUN" == "true" ]]; then
        log_info "[DRY-RUN] Would update .env with:"
        echo "  AGENT_CORE_MEMORY_ID=$memory_id"
        echo "  AGENT_CORE_MEMORY_STRATEGY_ID=$strategy_id"
        echo "  S3_BUCKET_NAME=$bucket_name"
        return 0
    fi

    # Create .env from example if it doesn't exist
    if [[ ! -f "$ENV_FILE" ]]; then
        if [[ -f "$SAMPLES_DIR/.env.example" ]]; then
            cp "$SAMPLES_DIR/.env.example" "$ENV_FILE"
            log_info "Created .env from .env.example"
        else
            touch "$ENV_FILE"
        fi
    fi

    # Update or add each variable
    update_env_var() {
        local key="$1"
        local value="$2"
        if grep -q "^${key}=" "$ENV_FILE" 2>/dev/null; then
            # Update existing value (macOS and Linux compatible)
            if [[ "$(uname)" == "Darwin" ]]; then
                sed -i '' "s|^${key}=.*|${key}=${value}|" "$ENV_FILE"
            else
                sed -i "s|^${key}=.*|${key}=${value}|" "$ENV_FILE"
            fi
        else
            # Append new value
            echo "${key}=${value}" >> "$ENV_FILE"
        fi
    }

    [[ -n "$memory_id" ]] && update_env_var "AGENT_CORE_MEMORY_ID" "$memory_id"
    [[ -n "$strategy_id" ]] && update_env_var "AGENT_CORE_MEMORY_STRATEGY_ID" "$strategy_id"
    [[ -n "$bucket_name" ]] && update_env_var "S3_BUCKET_NAME" "$bucket_name"

    log_info "Updated .env successfully"
}

# Main execution
main() {
    echo "========================================"
    echo "  AgentCore Setup Script"
    echo "========================================"
    echo "Region:      $AWS_REGION"
    echo "Name prefix: $NAME_PREFIX"
    echo "Dry run:     $DRY_RUN"
    echo ""

    verify_aws_credentials

    local memory_id=""
    local strategy_id=""
    local bucket_name=""

    # Create memory
    if [[ "$SKIP_MEMORY" != "true" ]]; then
        local memory_output
        memory_output=$(create_memory)
        memory_id=$(echo "$memory_output" | grep "^MEMORY_ID=" | cut -d= -f2)
        strategy_id=$(echo "$memory_output" | grep "^STRATEGY_ID=" | cut -d= -f2)

        if [[ -n "$memory_id" ]]; then
            log_info "Memory ID: $memory_id"
            log_info "Strategy ID: $strategy_id"
        else
            log_error "Failed to create or find memory"
            exit 1
        fi
    else
        log_warn "Skipping memory creation"
    fi

    # Create S3 bucket
    if [[ "$SKIP_S3" != "true" ]]; then
        local bucket_output
        bucket_output=$(create_s3_bucket)
        bucket_name=$(echo "$bucket_output" | grep "^BUCKET_NAME=" | cut -d= -f2)

        if [[ -n "$bucket_name" ]]; then
            log_info "Bucket: $bucket_name"
        else
            log_error "Failed to create or find S3 bucket"
            exit 1
        fi
    else
        log_warn "Skipping S3 bucket creation"
    fi

    # Update .env
    update_env_file "$memory_id" "$strategy_id" "$bucket_name"

    echo ""
    echo "========================================"
    echo "  Setup Complete"
    echo "========================================"
    echo ""
    echo "Run the agent_core sample with:"
    echo "  uv run strands agent_core"
    echo ""
}

main
