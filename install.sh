#!/bin/bash

# tunnel-rs installer for Linux and Mac
# Downloads latest binary from: https://github.com/andrewtheguy/tunnel-rs/releases
#
# Usage: ./install.sh [RELEASE_TAG] [--prerelease]
# Or set RELEASE_TAG environment variable

set -e

REPO_OWNER="andrewtheguy"
REPO_NAME="tunnel-rs"
DOWNLOAD_ONLY=false
PREFER_PRERELEASE=false

# Color output (defined early for use in release tag helpers)
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to print colored messages
print_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

print_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Fetch the latest stable release tag (non-prerelease)
get_latest_release_tag() {
    local api_url="https://api.github.com/repos/${REPO_OWNER}/${REPO_NAME}/releases/latest"
    local release_json

    if command -v curl >/dev/null 2>&1; then
        release_json=$(curl -s "$api_url")
    elif command -v wget >/dev/null 2>&1; then
        release_json=$(wget -qO- "$api_url")
    else
        print_error "Neither curl nor wget is available. Please install one of them."
        exit 1
    fi

    local tag
    tag=$(echo "$release_json" | grep -m1 '"tag_name"' | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')

    if [ -z "$tag" ]; then
        print_error "Could not find a latest release on GitHub"
        exit 1
    fi

    echo "$tag"
}

# Fetch the latest prerelease tag
get_latest_prerelease_tag() {
    local api_url="https://api.github.com/repos/${REPO_OWNER}/${REPO_NAME}/releases?per_page=30"
    local releases_json

    if command -v curl >/dev/null 2>&1; then
        releases_json=$(curl -s "$api_url")
    elif command -v wget >/dev/null 2>&1; then
        releases_json=$(wget -qO- "$api_url")
    else
        print_error "Neither curl nor wget is available. Please install one of them."
        exit 1
    fi

    # Capture the first prerelease entry and return its tag_name
    local tag
    tag=$(echo "$releases_json" | awk '
        /"tag_name"/ {gsub(/[,"]/, "", $2); tag=$2}
        /"prerelease": *true/ {if(tag!=""){print tag; exit}}
    ')

    if [ -z "$tag" ]; then
        print_error "Could not find any prerelease on GitHub"
        exit 1
    fi

    echo "$tag"
}

# Fetch full release info (including asset checksums) from GitHub API
get_release_info() {
    local tag="$1"
    local api_url="https://api.github.com/repos/${REPO_OWNER}/${REPO_NAME}/releases/tags/${tag}"

    if command -v curl >/dev/null 2>&1; then
        curl -s "$api_url"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO- "$api_url"
    else
        print_error "Neither curl nor wget is available."
        return 1
    fi
}

# Extract SHA-256 checksum from release JSON for a specific binary
get_expected_checksum() {
    local release_json="$1"
    local binary_name="$2"

    # Extract sha256 hash for matching asset
    # The digest field appears ~35 lines after the name field due to nested uploader object
    echo "$release_json" | grep -A40 "\"name\": \"${binary_name}\"" | \
        grep '"digest"' | head -1 | grep -o 'sha256:[a-f0-9]*' | cut -d: -f2
}

# Compute SHA-256 checksum of a file (cross-platform)
compute_checksum() {
    local file="$1"

    if command -v sha256sum >/dev/null 2>&1; then
        # Linux
        sha256sum "$file" | cut -d' ' -f1
    elif command -v shasum >/dev/null 2>&1; then
        # macOS
        shasum -a 256 "$file" | cut -d' ' -f1
    else
        print_error "No SHA-256 tool available (need sha256sum or shasum)"
        return 1
    fi
}

# Verify file checksum against expected value
verify_checksum() {
    local file="$1"
    local expected="$2"

    print_info "Verifying checksum..."
    local actual
    actual=$(compute_checksum "$file")

    if [ $? -ne 0 ]; then
        return 1
    fi

    if [ "$expected" = "$actual" ]; then
        print_info "Checksum verified: ${actual:0:16}..."
        return 0
    else
        print_error "Checksum verification FAILED!"
        print_error "Expected: $expected"
        print_error "Actual:   $actual"
        return 1
    fi
}

# Parse command-line arguments
parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --download-only)
                DOWNLOAD_ONLY=true
                shift
                ;;
            --prerelease)
                PREFER_PRERELEASE=true
                shift
                ;;
            --help|-h)
                show_usage
                exit 0
                ;;
            *)
                # Assume it's a release tag
                RELEASE_TAG="$1"
                shift
                ;;
        esac
    done

    # If RELEASE_TAG not set via args, check environment variable or fetch latest
    if [ -z "$RELEASE_TAG" ]; then
        if [ -n "${RELEASE_TAG_ENV:-}" ]; then
            RELEASE_TAG="$RELEASE_TAG_ENV"
        else
            if [ "$PREFER_PRERELEASE" = true ]; then
                print_info "Fetching latest prerelease tag from GitHub..."
                RELEASE_TAG=$(get_latest_prerelease_tag)
            else
                print_info "Fetching latest release tag from GitHub..."
                RELEASE_TAG=$(get_latest_release_tag)
            fi
        fi
    fi
}

# Detect OS
detect_os() {
    case "$(uname -s)" in
        Linux*)
            OS="linux"
            ;;
        Darwin*)
            OS="macos"
            ;;
        *)
            print_error "Unsupported operating system: $(uname -s)"
            print_error "This script only supports Linux and macOS"
            exit 1
            ;;
    esac
}

# Detect architecture
detect_arch() {
    ARCH=$(uname -m)
    case $ARCH in
        x86_64|amd64)
            ARCH="amd64"
            ;;
        aarch64|arm64)
            ARCH="arm64"
            ;;
        *)
            print_error "Unsupported architecture: $ARCH"
            print_error "Supported architectures: x86_64/amd64, aarch64/arm64"
            exit 1
            ;;
    esac
}

# Map OS and architecture to binary name
get_binary_name() {
    case "${OS}-${ARCH}" in
        "linux-amd64")
            BINARY_NAME="tunnel-rs-linux-amd64"
            ;;
        "linux-arm64")
            BINARY_NAME="tunnel-rs-linux-arm64"
            ;;
        "macos-arm64")
            BINARY_NAME="tunnel-rs-macos-arm64"
            ;;
        *)
            print_error "Unsupported platform: ${OS}-${ARCH}"
            print_error "Supported platforms:"
            print_error "  - linux-amd64 (x86_64 Linux)"
            print_error "  - linux-arm64 (aarch64 Linux)"
            print_error "  - macos-arm64 (Apple Silicon Mac)"
            exit 1
            ;;
    esac
}

# Download binary and verify checksum
download_binary() {
    local base_url="https://github.com/${REPO_OWNER}/${REPO_NAME}/releases/download/${RELEASE_TAG}"
    local url="${base_url}/${BINARY_NAME}"
    local output_path="$1"

    print_info "Downloading ${BINARY_NAME} from ${url}"

    # Download the binary
    if command -v curl >/dev/null 2>&1; then
        if ! curl -fL -o "$output_path" "$url"; then
            print_error "Failed to download binary"
            exit 1
        fi
    elif command -v wget >/dev/null 2>&1; then
        if ! wget -O "$output_path" "$url"; then
            print_error "Failed to download binary"
            exit 1
        fi
    else
        print_error "Neither curl nor wget is available. Please install one of them."
        exit 1
    fi

    # Verify checksum
    if [ -z "$EXPECTED_CHECKSUM" ]; then
        print_error "No checksum available. Aborting."
        rm -f "$output_path"
        exit 1
    fi
    if ! verify_checksum "$output_path" "$EXPECTED_CHECKSUM"; then
        print_error "Binary integrity check failed. Aborting."
        rm -f "$output_path"
        exit 1
    fi
}

# Download only - save to current directory
download_only() {
    local output_file="./${BINARY_NAME}"

    download_binary "$output_file"

    # Make executable
    chmod +x "$output_file"

    # Test the binary
    print_info "Testing downloaded binary..."
    local version_info
    if ! version_info=$("$output_file" --version 2>&1); then
        print_error "Binary test failed. The downloaded file may be corrupted or incompatible."
        print_error "Output: $version_info"
        rm -f "$output_file"
        exit 1
    fi

    print_info "Binary test successful: $version_info"
    print_info "Binary saved to: ${output_file}"
}

# Check if sourcing a profile would add target_dir to PATH
check_profile_has_path() {
    local profile="$1"
    local target_dir="$2"

    if [ -z "$profile" ] || [ ! -f "$profile" ]; then
        return 1
    fi

    # Source the profile in a subshell and check if target_dir is in PATH
    local new_path
    new_path=$(HOME="$HOME" SHELL="$SHELL" bash -c "source '$profile' 2>/dev/null; echo \"\$PATH\"" 2>/dev/null)

    if [[ ":$new_path:" == *":$target_dir:"* ]]; then
        return 0
    fi
    return 1
}

# Find a profile file that has .local/bin configured
find_profile_with_local_bin() {
    local target_dir="$1"
    local shell_name
    shell_name=$(basename "$SHELL")

    # List of profile files to check (order matters - check login profiles first)
    local profiles=()

    case "$shell_name" in
        bash)
            profiles=("$HOME/.bash_profile" "$HOME/.profile" "$HOME/.bashrc")
            ;;
        zsh)
            profiles=("$HOME/.zprofile" "$HOME/.zshrc")
            ;;
        *)
            profiles=("$HOME/.profile")
            ;;
    esac

    # Find first profile that has .local/bin configured
    for profile in "${profiles[@]}"; do
        if check_profile_has_path "$profile" "$target_dir"; then
            echo "$profile"
            return 0
        fi
    done

    return 1
}

# Download binary to temporary location, test it, and install
download_and_install() {
    local temp_dir
    temp_dir=$(mktemp -d)
    local temp_binary="${temp_dir}/${BINARY_NAME}"
    local final_path="$HOME/.local/bin/tunnel-rs"

    # Set up trap to clean up temp directory on exit
    trap 'rm -rf "$temp_dir"' EXIT

    download_binary "$temp_binary"

    # Make executable
    chmod +x "$temp_binary"

    # Test the binary
    print_info "Testing downloaded binary..."
    local version_info
    if ! version_info=$("$temp_binary" --version 2>&1); then
        print_error "Binary test failed. The downloaded file may be corrupted or incompatible."
        print_error "Output: $version_info"
        exit 1
    fi

    print_info "Binary test successful: $version_info"

    # Create target directory if it doesn't exist
    local target_dir="$HOME/.local/bin"
    mkdir -p "$target_dir"

    # Move the tested binary to final location
    if ! mv "$temp_binary" "$final_path"; then
        print_error "Failed to move binary to final location"
        exit 1
    fi

    # Clean up temp directory (trap will also handle this)
    rm -rf "$temp_dir"

    print_info "Binary installed successfully to ${final_path}"

    # Check PATH and suggest how to fix if needed
    if [[ ":$PATH:" != *":$target_dir:"* ]]; then
        local profile
        profile=$(find_profile_with_local_bin "$target_dir")

        if [ -n "$profile" ]; then
            # Profile already has .local/bin configured, just needs reload
            print_warn "${target_dir} is not in your current PATH, but is configured in your profile."
            print_warn "To use tunnel-rs now, reload your profile:"
            echo ""
            echo "    source $profile"
            echo ""
            print_warn "Or start a new terminal session."
        else
            # Profile doesn't have .local/bin, need to add it
            print_warn "${target_dir} is not in your PATH"
            print_warn "Add the following line to your shell profile (.bashrc, .zshrc, etc.):"
            echo ""
            echo "    export PATH=\"\$HOME/.local/bin:\$PATH\""
            echo ""
            print_warn "Then reload your profile or start a new terminal session."
        fi
    fi
}

# Display usage information
show_usage() {
    echo "Usage: $0 [OPTIONS] [RELEASE_TAG]"
    echo ""
    echo "Download and install tunnel-rs binary"
    echo ""
    echo "Options:"
    echo "  --download-only  Download binary to current directory without installing"
    echo "  --prerelease     Use latest prerelease instead of latest stable release"
    echo "  -h, --help       Show this help message"
    echo ""
    echo "Arguments:"
    echo "  RELEASE_TAG      GitHub release tag to download (default: latest)"
    echo ""
    echo "Examples:"
    echo "  $0                              # Install latest release"
    echo "  $0 20251210172710               # Install specific release"
    echo "  $0 --prerelease                 # Install latest prerelease"
    echo "  $0 --download-only              # Download latest to current directory"
    echo "  $0 --download-only 20251210172710  # Download specific release"
    echo ""
    echo "Supported platforms: Linux (amd64, arm64), macOS (arm64)"
}

# Main installation function
install() {
    if [ "$DOWNLOAD_ONLY" = true ]; then
        print_info "tunnel-rs downloader"
    else
        print_info "tunnel-rs installer"
    fi
    print_info "Release: ${RELEASE_TAG}"
    print_info "Repository: ${REPO_OWNER}/${REPO_NAME}"

    detect_os
    detect_arch
    get_binary_name

    print_info "Platform detected: ${OS}-${ARCH}"
    print_info "Binary name: ${BINARY_NAME}"

    # Fetch release info for checksum verification
    print_info "Fetching release information..."
    RELEASE_JSON=$(get_release_info "$RELEASE_TAG")

    if [ -z "$RELEASE_JSON" ] || echo "$RELEASE_JSON" | grep -q '"message": "Not Found"'; then
        print_error "Could not fetch release info from GitHub. Cannot verify binary integrity."
        exit 1
    fi

    EXPECTED_CHECKSUM=$(get_expected_checksum "$RELEASE_JSON" "$BINARY_NAME")
    if [ -z "$EXPECTED_CHECKSUM" ]; then
        print_error "No checksum found for ${BINARY_NAME} in release. Cannot verify binary integrity."
        exit 1
    fi
    print_info "Expected checksum: ${EXPECTED_CHECKSUM:0:16}..."

    if [ "$DOWNLOAD_ONLY" = true ]; then
        download_only
        print_info "Download completed successfully!"
    else
        download_and_install
        print_info "Installation completed successfully!"
        print_info "You can now run 'tunnel-rs' from your terminal."
    fi
}

# Check if running with proper privileges
check_privileges() {
    if [ "$EUID" -eq 0 ]; then
        print_warn "Running as root. It's recommended to install as a regular user."
    fi
}

# Main execution
main() {
    parse_args "$@"

    if [ "$DOWNLOAD_ONLY" = true ]; then
        print_info "Starting tunnel-rs download..."
    else
        print_info "Starting tunnel-rs installation..."
        check_privileges
    fi

    install
}

# Run main function
main "$@"
