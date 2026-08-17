#!/usr/bin/env -S uv run -q --script
# /// script
# requires-python = ">=3.10"
# dependencies = ["cryptography>=42"]
# ///
"""Generate an Ed25519 client authentication key in the compact
app-independent format:

  private: ed25519-sec:<unpadded url-safe base64 of the 32-byte seed>
  public:  ed25519-pub:<unpadded url-safe base64 of the 32-byte public key>

Usage: generate-auth-key.py [comment] [-o FILE] [--force] [--json]

By default the private-key file is written to stdout, so save it with
`generate-auth-key.py "alice laptop" > client.key`. When stdout is
redirected, the authorized-keys entry is also printed to stderr so it stays
visible; append it to the server's authorized_keys file. With --output the
key file is written with 0600 permissions and the entry goes to stdout
instead; --json prints both halves as one JSON object for automation.
show-auth-key re-derives the entry from an existing key file.
"""

import argparse
import base64
import json
import os
import sys
from datetime import datetime, timezone
from pathlib import Path

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

PRIVATE_KEY_PREFIX = "ed25519-sec:"
PUBLIC_KEY_PREFIX = "ed25519-pub:"


def b64url(raw: bytes) -> str:
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Generate an Ed25519 client authentication key."
    )
    parser.add_argument(
        "comment",
        nargs="?",
        default="",
        help="comment appended to the printed authorized-key entry",
    )
    parser.add_argument(
        "-o",
        "--output",
        help='path where to save the private key file ("-" means stdout)',
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="overwrite an existing private key file (requires --output)",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="print the authorized-key entry and private key as JSON "
        "instead of a key file",
    )
    args = parser.parse_args()
    if args.json and args.output is not None:
        parser.error("--json conflicts with --output")
    if args.force and args.output is None:
        parser.error("--force requires --output")

    # The comment runs to end of line in authorized_keys, so a line break
    # would split the entry and forge a second authorized key.
    comment = args.comment.strip()
    if "\n" in comment or "\r" in comment:
        parser.error("comment must not contain line breaks")

    key = Ed25519PrivateKey.generate()
    sec = PRIVATE_KEY_PREFIX + b64url(key.private_bytes_raw())
    pub = PUBLIC_KEY_PREFIX + b64url(key.public_key().public_bytes_raw())
    entry = f"{pub} {comment}" if comment else pub

    if args.json:
        print(json.dumps({"authorized_key": entry, "private_key": sec}))
        return

    created = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    key_file = (
        "# Ed25519 client authentication key\n"
        f"# Created: {created}\n"
        f"# Public key: {entry}\n"
        f"{sec}\n"
    )

    if args.output is None or args.output == "-":
        sys.stdout.write(key_file)
        if not sys.stdout.isatty():
            print(entry, file=sys.stderr)
        return

    path = Path(args.output).expanduser()
    if path.exists() and not args.force:
        sys.exit(f"File already exists: {path}. Use --force to overwrite.")
    path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        # The mode above only applies when the file is created, so tighten
        # the permissions of a pre-existing (--force) file too. Windows has
        # no fchmod; there the mode is advisory anyway.
        if hasattr(os, "fchmod"):
            os.fchmod(fd, 0o600)
        f.write(key_file)
    print(f"Authentication private key saved to: {path}", file=sys.stderr)
    print(entry)


if __name__ == "__main__":
    main()
