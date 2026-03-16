#!/bin/bash
set -e

# Build script for cross-compiling Linux binaries using Docker
# Builds for both AMD64 and ARM64 architectures

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR="${SCRIPT_DIR}/target/build"
DOCKERFILE="${SCRIPT_DIR}/Dockerfile.build"

echo "tunnel-rs Cross-Compilation Build Script"
echo "========================================="

echo "Build directory: $BUILD_DIR"
echo "Dockerfile: $DOCKERFILE"

if ! command -v docker &> /dev/null; then
    echo "Error: Docker is not installed or not in PATH"
    exit 1
fi

if ! docker buildx version &> /dev/null; then
    echo "Error: Docker buildx is not available"
    exit 1
fi

mkdir -p "$BUILD_DIR"

BUILDER_NAME="tunnel-rs-builder"
if ! docker buildx inspect "$BUILDER_NAME" &> /dev/null; then
    docker buildx create --name "$BUILDER_NAME" --use --driver docker-container
else
    docker buildx use "$BUILDER_NAME"
fi

docker buildx build \
    --platform linux/amd64,linux/arm64 \
    --file "$DOCKERFILE" \
    --target export \
    --output type=local,dest="$BUILD_DIR" \
    "$SCRIPT_DIR"

if [ -d "$BUILD_DIR/linux_amd64" ]; then
    [ -f "$BUILD_DIR/linux_amd64/tunnel-rs" ] && mv "$BUILD_DIR/linux_amd64/tunnel-rs" "$BUILD_DIR/tunnel-rs-linux-amd64"
fi

if [ -d "$BUILD_DIR/linux_arm64" ]; then
    [ -f "$BUILD_DIR/linux_arm64/tunnel-rs" ] && mv "$BUILD_DIR/linux_arm64/tunnel-rs" "$BUILD_DIR/tunnel-rs-linux-arm64"
fi

echo "Build complete"
ls -lh "$BUILD_DIR"/tunnel-rs-* 2>/dev/null || true
