# Sourced helper: locate the flexaccess-keys CLI used for client
# authentication keys, setting KEYS_BIN.
#
# Resolution order: $FLEXACCESS_KEYS_BIN, PATH, then a download of the pinned
# release into the work directory passed as $1.

FLEXACCESS_KEYS_VERSION="v0.0.2"

resolve_flexaccess_keys_bin() {
    local work="$1"
    if [[ -n "${FLEXACCESS_KEYS_BIN:-}" ]]; then
        KEYS_BIN="$FLEXACCESS_KEYS_BIN"
    elif command -v flexaccess-keys >/dev/null 2>&1; then
        KEYS_BIN="$(command -v flexaccess-keys)"
    else
        local suffix sha256 actual
        case "$(uname -s)-$(uname -m)" in
            Linux-x86_64)
                suffix="linux-amd64"
                sha256="b59d6086a9107cd0af8ff560806ec3543e8247e826ec15552834528541c166b3"
                ;;
            Linux-aarch64 | Linux-arm64)
                suffix="linux-arm64"
                sha256="72db6893c6acb19ba479a9ba84e2f5cb623e08f58301f5d4d6ffe3ecb2c798cc"
                ;;
            Darwin-arm64)
                suffix="macos-arm64"
                sha256="38eae62285f53dbd43e1248e01684036313b70c63b949b7b2ba99a0087e18c39"
                ;;
            *)
                echo "ERROR: no flexaccess-keys release binary for $(uname -s)/$(uname -m); set FLEXACCESS_KEYS_BIN" >&2
                return 1
                ;;
        esac
        KEYS_BIN="$work/flexaccess-keys"
        curl -fsSL -o "$KEYS_BIN" \
            "https://github.com/flexaccessdev/flexaccess-keys/releases/download/$FLEXACCESS_KEYS_VERSION/flexaccess-keys-$suffix"
        if command -v sha256sum >/dev/null 2>&1; then
            actual="$(sha256sum "$KEYS_BIN" | awk '{print $1}')"
        else
            actual="$(shasum -a 256 "$KEYS_BIN" | awk '{print $1}')"
        fi
        if [[ "$actual" != "$sha256" ]]; then
            echo "ERROR: checksum mismatch for flexaccess-keys-$suffix: expected $sha256, got $actual" >&2
            rm -f "$KEYS_BIN"
            return 1
        fi
        chmod +x "$KEYS_BIN"
    fi
    if ! "$KEYS_BIN" --version >/dev/null; then
        echo "ERROR: flexaccess-keys binary at $KEYS_BIN is not runnable" >&2
        return 1
    fi
}
