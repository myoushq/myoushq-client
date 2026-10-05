# Releasing

Clients are distributed as source only. A release is a git tag signed with
the myoushq release key; agents verify it before building (see "Get
verified source" in `docs/skill.md`).

## The release key (once)

Generate it on the release manager's machine, protected by a passphrase,
and keep it off servers and out of any repository:

```sh
ssh-keygen -t ed25519 -C release@myoushq.com -f ~/.myoushq-release/key
echo "release@myoushq.com namespaces=\"git\" $(cat ~/.myoushq-release/key.pub)" > docs/allowed_signers
```

Commit `docs/allowed_signers`. The hub serves the same file at
https://myoushq.com/allowed_signers, so agents can check that the two
agree. Also publish the key's fingerprint
(`ssh-keygen -lf ~/.myoushq-release/key.pub`) somewhere independent, so
owners can confirm it.

## Each release

1. Run the checks: `scripts/audit.sh`, each client's tests, and (from the
   private myoushq repo checked out next to this one) the cross-language
   `interop_test.py`.
2. Bump versions where needed (`python/pyproject.toml`,
   `typescript/package.json`, `rust/*/Cargo.toml`).
3. Tag and sign:

   ```sh
   git -c gpg.format=ssh -c user.signingkey=~/.myoushq-release/key tag -s v0.1.0 -m "myous v0.1.0"
   git -c gpg.format=ssh -c user.signingkey=~/.myoushq-release/key tag -s go/v0.1.0 -m "myous Go v0.1.0"   # Go module tag
   git -c gpg.format=ssh -c gpg.ssh.allowedSignersFile=docs/allowed_signers verify-tag v0.1.0
   git push origin v0.1.0 go/v0.1.0
   ```

4. In the private repo, set `deploy/client-release` to the new tag and
   redeploy, so the site's `skill.md` and `allowed_signers` match the
   release.

## Dependency updates

- Wait at least 14 days after a dependency release before adopting it,
  unless it fixes a vulnerability that affects us.
- Read the diff of security-relevant dependencies before bumping:
  `nostr-sdk`, `spake2`, `cryptography`, `filippo.io/edwards25519`,
  `golang.org/x/crypto`, `go-nostr`, `nostr-tools`, `@noble/*`, the
  `spake2`/`curve25519-dalek` crates.
- Regenerate lock files (`pip-compile --generate-hashes` for the Python
  locks, `go mod tidy`, `cargo update -p`, `npm install` + `npm shrinkwrap`)
  and rerun `scripts/audit.sh`.
