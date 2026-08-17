no backward compatibility is needed since it is still pre-release.
run cargo clippy and cargo test -q after making changes.
no cargo fmt

# Design notes
Ed25519 authentication key management (key format, key files, authorized-keys
parsing, and the generate-auth-key / show-auth-key CLI) lives in
https://github.com/flexaccessdev/flexaccess-keys. Do not add key-management
commands or reimplement the key format here; tunnel-rs owns only its
domain-separated challenge transcript (src/auth.rs) and consumes the
flexaccess-keys crate for everything else. This does not cover the iroh server
identity key (generate-server-key, src/secret.rs), which stays in tunnel-rs.

The iroh transport layer shared with ezvpn and flextunnel — relays and address
lookup, the per-relay startup probe, relay auth tokens, relay self-hosting — is
documented once in https://github.com/flexaccessdev/iroh-common-architecture. Do
not duplicate it in this repo; update it there and link to it. tunnel-rs is the
reference program for relay-only setups, so relay-only behavior changes should be
reflected there too.