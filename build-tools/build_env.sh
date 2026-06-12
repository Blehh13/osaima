#!/bin/bash

# Abort on error
set -e

# Detect container engine (podman preferred on Fedora, docker fallback)
if command -v podman &> /dev/null; then
    CONTAINER_ENGINE="podman"
elif command -v docker &> /dev/null; then
    CONTAINER_ENGINE="docker"
else
    echo "Error: Neither podman nor docker found. Please install one to continue."
    exit 1
fi

IMAGE_NAME="interstellar-os-builder"
PROJECT_ROOT=$(realpath $(dirname "$0")/..)

echo "==> Building container image using $CONTAINER_ENGINE..."
$CONTAINER_ENGINE build -t $IMAGE_NAME "$PROJECT_ROOT/build-tools"

echo "==> Container image $IMAGE_NAME built successfully."

# If a script is provided, run it inside the container. Otherwise, drop to shell.
if [ -n "$1" ]; then
    SCRIPT_PATH=$1
    # If the script is not a direct path, look for it in build-tools/scripts
    if [[ ! -f "$SCRIPT_PATH" ]]; then
        # Try relative to project root
        SCRIPT_PATH="$PROJECT_ROOT/build-tools/scripts/$1"
    fi
    if [[ ! -f "$SCRIPT_PATH" ]]; then
        echo "Error: Script $SCRIPT_PATH not found."
        exit 1
    fi
    # Translate host path to container path
    BASENAME=$(basename "$SCRIPT_PATH")
    
    echo "==> Running script $BASENAME inside container..."
    $CONTAINER_ENGINE run --rm -v "$PROJECT_ROOT:/osaima-build:Z" -it $IMAGE_NAME /bin/bash "/osaima-build/build-tools/scripts/$BASENAME"
else
    echo "==> Dropping into interactive build shell..."
    $CONTAINER_ENGINE run --rm -v "$PROJECT_ROOT:/osaima-build:Z" -it $IMAGE_NAME /bin/bash
fi
