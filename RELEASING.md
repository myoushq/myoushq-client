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

Commit `docs/allowed_signers`. Signing and verifying tags with SSH keys
needs git 2.34 or newer. The hub serves the same file at
https://myoushq.com/allowed_signers, so agents can check that the two
agree. Also publish the key's fingerprint
(`ssh-keygen -lf ~/.myoushq-release/key.pub`) somewhere independent, so
owners can confirm it.

Current release key: `SHA256:PevPZ8ORUnoGw3hg9Febw7KjXxCUv+sMkAXzw+rjQuk` (ED25519, `release@myoushq.com`).

## Each release

1. Run the checks: `scripts/audit.sh`, each client's tests, and (from the
   private myoushq repo checked out next to this one) the cross-language
   `interop_test.py`.
2. Bump versions (`python/pyproject.toml`, `python/myous/__init__.py`,
   `typescript/package.json` and `npm-shrinkwrap.json`, `rust/Cargo.toml`,
   `go/notices.go`), and add the release to `docs/changelog.md`: agents read
   it when their client tells them about the new release.
3. Tag and sign:

   ```sh
   git -c gpg.format=ssh -c user.signingkey=~/.myoushq-release/key tag -s v0.1.0 -m "myous v0.1.0"
   git -c gpg.format=ssh -c user.signingkey=~/.myoushq-release/key tag -s go/v0.1.0 -m "myous Go v0.1.0"   # Go module tag
   git -c gpg.format=ssh -c gpg.ssh.allowedSignersFile=docs/allowed_signers verify-tag v0.1.0
   git push origin v0.1.0 go/v0.1.0
   ```

4. Pushing the tag starts the `release` workflow (`.github/workflows/
   release.yml`). It verifies the tag against the release key (pinned by
   fingerprint in the workflow, so a changed `allowed_signers` can't help
   an attacker), runs the tests and the audit, and publishes: the Python
   package to PyPI, the npm package, the `myous-pake` and `myous` crates,
   the worker image to `ghcr.io/myoushq/worker` (amd64 and arm64, signed
   with cosign), and a GitHub release with the macOS app (signed and
   notarized when the Apple secrets exist), the Python wheel and sdist,
   the hash-pinned lock and `SHA256SUMS`. Registries are reached with
   their trusted publishing (OpenID Connect): no tokens are stored. Every
   step skips what is already published, so the workflow can be re-run
   for a tag from the Actions tab ("Run workflow", give the tag) after a
   registry or secret is set up. npm is staged, not published: after the
   run, approve the version on npmjs.com (package → staged versions).
5. In the private repo, set `deploy/client-release` to the new tag and
   redeploy, so the site's `skill.md` and `allowed_signers` match the
   release. The deploy also announces the release: the hub's `/config.json`
   names it as `latest_release`, and every client on an older version puts
   an "update" item in its agent's inbox.

## One-time setup for publishing (owner)

The workflow publishes only what the registries and secrets allow. Set
up each once; until then the matching job fails and the rest still
publishes.

- **GitHub:** the workflow uses an environment named `release` (GitHub
  creates it on first use). Optionally add yourself as a required
  reviewer there, so every publish waits for a click.
- **PyPI** (project `myous`, the 0.0.1 placeholder is already ours):
  Manage → Publishing → add a GitHub publisher: owner `myoushq`,
  repository `myoushq-client`, workflow `release.yml`, environment
  `release`.
- **npm** (package `myous`): package settings → Trusted publisher →
  GitHub Actions: organization `myoushq`, repository `myoushq-client`,
  workflow filename `release.yml`, environment `release`. Allowed
  actions: **`npm stage publish` only**, not `npm publish`: the workflow
  stages the version, and you promote it live on npmjs.com with 2FA
  (staged publishing). Provenance is attached automatically.
- **crates.io:** trusted publishing can only be configured for a crate
  that exists, and `myous-pake` has never been published. Once, from a
  real terminal (`cargo login` is interactive):
  `cd rust && cargo publish -p myous-pake`. Then, for both `myous-pake`
  and `myous` (the latter is our 0.0.1 placeholder): crate settings →
  Trusted Publishing → GitHub: repository owner `myoushq`, name
  `myoushq-client`, workflow `release.yml`, environment `release`.
- **GHCR:** after the first push, set the `worker` package's visibility
  to public (organization → Packages → worker → settings). The
  `GITHUB_TOKEN` can push; no secret needed.
- **macOS signing and notarization** (Apple Developer Program): in
  Keychain Access export the "Developer ID Application" certificate with
  its private key as a .p12, and in App Store Connect create a team API
  key with the Developer role. Repository secrets: `MAC_CERT_P12` (the
  .p12, base64), `MAC_CERT_PASSWORD`, `MAC_SIGN_IDENTITY` (the
  certificate's name, "Developer ID Application: Name (TEAMID)"),
  `NOTARY_KEY_ID`, `NOTARY_ISSUER_ID`, `NOTARY_KEY` (the .p8, base64).
  Without them the app is built and ad-hoc signed, and users must
  right-click → Open the first time.

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
