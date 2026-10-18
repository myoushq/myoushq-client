# Releasing

A release is a git tag signed with the myoushq release key. CI publishes
each tag to PyPI, crates.io, GHCR (the worker image) and a GitHub release
(the apps, the Python packages and locks), and stages it on npm for the
owner to promote (step 4). Agents install the exact published version,
or verify the tag and build ("Install a published package" and "Or build
from verified source" in `docs/skill.md`).

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

0. Once: `git config core.hooksPath scripts/hooks`. The pre-push hook then
   refuses to push a `v*` tag whose versions don't match (every file of
   the bump list in step 2, checked by `scripts/check-versions.sh`, which
   the release workflow runs too), that isn't signed by the release key,
   or while `scripts/audit.sh` reports a problem (new advisories appear
   between releases; the usual fix is a toolchain or dependency bump).
   `MYOUS_SKIP_HOOK=1 git push` skips it.
1. Run the checks: `scripts/audit.sh`, each client's tests, and (with the
   private myoushq repo checked out next to this one, for the hub) the
   cross-language `interop_test.py`.
2. Bump versions (`python/pyproject.toml`, `python/myous/__init__.py`,
   `typescript/package.json` and `npm-shrinkwrap.json` (two places),
   `rust/Cargo.toml`, the `myous-pake` dependency in `rust/myous/Cargo.toml`
   and `rust/wasm/Cargo.toml`, `rust/Cargo.lock`, `go/notices.go`,
   `worker/desktop/package.json` and `worker/desktop/src-tauri/Cargo.toml`
   with its `Cargo.lock`; the apps for Mac, Linux and Windows all take
   `worker/desktop/package.json`'s), the install lines in `go/README.md`
   and `rust/README.md`, and add the release to `docs/changelog.md`:
   agents read it when their client tells them about the new release.
   A release's date there is its tag's day in UTC.
   `scripts/check-versions.sh X.Y.Z` checks every file of this list (the
   changelog aside); a file that starts carrying the version goes into
   both.
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
   an attacker) and the versions in the sources against the tag, runs the
   tests and the audit, and publishes: the Python package to PyPI, the npm
   package, the `myous-pake` and `myous` crates, the worker image to
   `ghcr.io/myoushq/worker` (amd64 and arm64, signed with cosign), and a
   GitHub release with myous for Mac (`myous-<tag>.dmg`:
   `worker/desktop/make-mac.sh --universal` on a macOS runner, then
   `worker/mac/notarize.sh` and `make-dmg.sh`; signed and notarized when
   the Apple secrets exist), myous for Linux (AppImage and `.deb`) and
   Windows (the installer), the Python wheel and sdist, the hash-pinned
   locks (`requirements-python.lock`, and `requirements-qr-python.lock`
   with `qrcode` for QR images) and `SHA256SUMS`. Each registry waits only for its own language's
   tests and the audit (PyPI and the image for the Python tests, crates.io
   for Rust's, npm for TypeScript's), so a failure elsewhere doesn't stop
   it; the apps wait for the Python tests, and only the GitHub release
   waits for every test, the audit, the apps and the worker image's tags
   (the apps pull `ghcr.io/myoushq/worker:X.Y.Z`). Registries are reached
   with their trusted publishing (OpenID Connect): no tokens are stored. Every
   step skips what is already published, so the workflow can be re-run for
   a tag from the Actions tab ("Run workflow", give the tag) after a
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
  that exists, so `myous-pake` 0.4.1 was published once by hand, from a
  real terminal (`cargo login` is interactive):
  `cd rust && cargo publish -p myous-pake`. Then both `myous-pake` and
  `myous` (the latter was our 0.0.1 placeholder) got a trusted publisher:
  crate settings → Trusted Publishing → GitHub: repository owner
  `myoushq`, name `myoushq-client`, workflow `release.yml`, environment
  `release`. CI has published both crates since v0.4.2.
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
  With them the release run notarizes and staples the disk image as well
  as the app (the step "notarize and staple the disk image" in
  `release.yml`) and validates it: the run fails unless `xcrun stapler
  validate` and `spctl` accept the image, so the download itself passes
  Gatekeeper, not only the app in it.
  Without them the app is built and ad-hoc signed, and users must
  right-click → Open the first time.

## Dependency updates

- Wait at least 14 days after a dependency release before adopting it,
  unless it fixes a vulnerability that affects us. That means a published
  advisory or CVE for the version we run, or a security fix (a timing fix
  included) in a code path we call, whether or not it has an advisory;
  look in the project's changelog, its GitHub security advisories and OSV
  (osv.dev). Fixes and hardening in code we don't call, with no advisory
  for our version, don't shorten the wait.
- Read the diff of security-relevant dependencies before bumping:
  `nostr-sdk`, `cryptography` (and `pycryptodome` on Intel Macs), `spake2`
  and its HKDF (`hkdf` on Intel Macs), `filippo.io/edwards25519`,
  `golang.org/x/crypto`, `go-nostr`, `nostr-tools`, `@noble/*`, the
  `spake2`/`curve25519-dalek` crates.
- Regenerate lock files (the Python locks as below, `go mod tidy`,
  `cargo update -p`, `npm install` + `npm shrinkwrap`) and rerun
  `scripts/audit.sh`, which audits every line of the Python locks
  whatever machine it runs on.
- The Python locks are universal (one lock for every platform), made
  with uv in `python/`:
  ```sh
  uv pip compile --universal --generate-hashes --python-version 3.9.2 pyproject.toml -o requirements.lock
  uv pip compile --universal --generate-hashes --python-version 3.9.2 --extra qr pyproject.toml -o requirements-qr.lock
  ```
  3.9.2, not 3.9: cryptography excludes 3.9.0 and 3.9.1, and uv would
  otherwise pin an older cryptography for them.

**The Intel Mac path.** `cryptography` has had no Intel Mac wheels since
49, so an Intel Mac would compile it for about ten minutes. The ciphers
and HKDF (`python/myous/_aead.py`) come from `cryptography` wherever it
is installed, since OpenSSL's AES uses the CPU's constant-time
instructions on ARM too while pycryptodome's ARM wheels are table-based,
and from `pycryptodome` on Intel Macs, where its AES-NI and CLMUL code
is constant-time. spake2 0.9 needs `cryptography` as well, so Intel Macs
take spake2 0.8 (the same protocol code on the pure-Python `hkdf`).
`pyproject.toml` gives each pair environment markers: one line where
`sys_platform` is `darwin` and `platform_machine` is `x86_64`, the other
everywhere else; the lock carries both, and pip installs the lines whose
markers match. The 0.9 line is bumped as usual; 0.8 is the last release
on `hkdf`, so its line stays as it is until the path is removed. CI's
`python-intel-mac-set` job runs the suite on the Intel Mac set, so in CI
`python/tests/vectors_test.py` and `pake_bytes_test.py` tie the two sets
together: both jobs check the same recorded bytes. `aead_test.py`'s
`BothBackends` compares the two backends directly but runs only where
both libraries are installed, never in CI: a local check, for a venv
with both. The path goes when Intel Macs are out of support (macOS 26
is the last for them; a TODO in the private repository).
