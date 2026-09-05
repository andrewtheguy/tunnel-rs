no backward compatibility or any legacy code paths.
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

That shared layer's code — `RelayConfig` and the relay probe, the address
lookup service custom relays require (its `lks1-` secret format and URL
layout), endpoint building and rebuild — lives in the `flexaccess-iroh` crate
(`../flexaccess-iroh`, consumed by git tag). Fix it there, tag a release, and
bump the tag here; never re-implement or fork a copy of it in this repo. Only
tunnel-rs-specific pieces (the `mf/4` ALPN, transport tuning, `--relay-only`
and its sequential relay dial, the challenge transcript, key files, the
`generate-lookup-secret` command) belong in `src/iroh_mode/endpoint.rs`,
`src/auth.rs`, and `src/secret.rs`.