# Sourced helper: locate the flexaccess-keys CLI used for client
# authentication keys, setting KEYS_BIN.
#
# Resolution order: $FLEXACCESS_KEYS_BIN, PATH, then a download of the pinned
# release into the work directory passed as $1.

FLEXACCESS_KEYS_VERSION="v0.0.1"

resolve_flexaccess_keys_bin() {
    local work="$1"
    if [[ -n "${FLEXACCESS_KEYS_BIN:-}" ]]; then
        KEYS_BIN="$FLEXACCESS_KEYS_BIN"
    elif command -v flexaccess-keys >/dev/null 2>&1; then
        KEYS_BIN="$(command -v flexaccess-keys)"
    else
        local suffix
        case "$(uname -s)-$(uname -m)" in
            Linux-x86_64) suffix="linux-amd64" ;;
            Linux-aarch64 | Linux-arm64) suffix="linux-arm64" ;;
            Darwin-arm64) suffix="macos-arm64" ;;
            *)
                echo "ERROR: no flexaccess-keys release binary for $(uname -s)/$(uname -m); set FLEXACCESS_KEYS_BIN" >&2
                return 1
                ;;
        esac
        KEYS_BIN="$work/flexaccess-keys"
        curl -fsSL -o "$KEYS_BIN" \
            "https://github.com/flexaccessdev/flexaccess-keys/releases/download/$FLEXACCESS_KEYS_VERSION/flexaccess-keys-$suffix"
        chmod +x "$KEYS_BIN"
    fi
    if ! "$KEYS_BIN" --version >/dev/null; then
        echo "ERROR: flexaccess-keys binary at $KEYS_BIN is not runnable" >&2
        return 1
    fi
}
