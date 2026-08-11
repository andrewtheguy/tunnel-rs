no backward compatibility is needed since it is still pre-release.
run cargo clippy and cargo test -q after making changes.
no cargo fmt

# Design notes
The iroh transport layer shared with ezvpn and flextunnel — relays and address
lookup, the per-relay startup probe, relay auth tokens, relay self-hosting — is
documented once in https://github.com/flexaccessdev/iroh-common-architecture. Do
not duplicate it in this repo; update it there and link to it. tunnel-rs is the
reference program for relay-only setups, so relay-only behavior changes should be
reflected there too.