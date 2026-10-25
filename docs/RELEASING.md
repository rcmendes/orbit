# Releasing Orbit

Orbit uses [cargo-dist](https://axodotdev.github.io/cargo-dist/) for release builds. The configuration lives in `dist-workspace.toml`; `.github/workflows/release.yml` is generated and should not be edited by hand.

## Released versions

- `v0.6.0` — latest
- `v0.5.0` — previous release
- `v0.4.0`
- `v0.3.0`
- `v0.2.0`
- `v0.1.0` — initial release

## One-time setup

1. Ensure the `@rcmendes` npm scope exists and is owned by the publishing account. No organization is needed when `rcmendes` is the npm username.
2. Create an npm granular access token with read/write access to `@rcmendes`.
3. Enable **Bypass 2FA** on the token. Without it, `npm publish` fails with `E403` even when the token has publish permission.
4. Add the token to GitHub Actions as the `NPM_TOKEN` repository secret:

   ```bash
   gh secret set NPM_TOKEN -R rcmendes/orbit
   ```

## Homebrew (gated)

Homebrew support is configured through cargo-dist but stays disabled until a tap exists:

1. Create `rcmendes/homebrew-tap`.
2. Ensure the release token can write to it.
3. Add `"homebrew"` to `installers`, add `"homebrew"` to `publish-jobs`, and set `tap = "rcmendes/homebrew-tap"` and `formula = "orbit"` in `dist-workspace.toml`.
4. Run `dist generate` and commit the regenerated workflow.

Until then, the README leads with the shell, PowerShell, and npm installers.

## Cut a release

1. Update `version` in `Cargo.toml` and add a matching heading to `../CHANGELOG.md` (for example `## 0.6.0 - 2026-10-25`). The changelog must stay at the repository root: cargo-dist only discovers release-note entries there.
2. Commit the changes.
3. Tag and push:

   ```bash
   git tag v0.6.0
   git push origin v0.6.0
   ```

4. The `Release` workflow:
   - plans and builds macOS arm64/x86_64, Linux x86_64/arm64, and Windows x86_64;
   - produces `.tar.xz`/`.zip` archives, shell and PowerShell installers, checksums, and the npm package;
   - creates the GitHub Release with changelog notes;
   - publishes `@rcmendes/orbit` to npm using `NPM_TOKEN`.

5. Trigger the post-release smoke check immediately after the Release workflow finishes:

   ```bash
   gh workflow run "Install smoke" --ref v0.6.0
   ```

   A release created by `GITHUB_TOKEN` does not emit a downstream workflow event, so `Install smoke` does not start automatically. It installs the latest public shell/PowerShell installers and the npm package, then exercises `orbit --version`, `orbit init`, `orbit add`, `orbit ready`, and `orbit setup --print`. A red run means the published artifacts are broken even though the release exists.

## Packaging checks

`scripts/onboarding-smoke.sh` and `scripts/onboarding-smoke.ps1` are the single onboarding sequence used by CI and available locally:

```bash
cargo build --release
ORBIT_BIN="$PWD/target/release/orbit" bash scripts/onboarding-smoke.sh
```

They must stay in sync with the documented quick start; do not add a second smoke sequence.

## npm notes

- The first publish of a new package can briefly create a `0.0.0-stage` placeholder while npm processes the upload. It is not the `latest` tag; the real version appears after processing.
- Push tags one at a time, oldest first, so npm's `latest` tag ends on the newest version.
- A published npm version is immutable. Fix a bad release by publishing a new version, not by moving its tag.

## Known issues

- The `v0.1.0` tag's CI check is red on Windows because it predates the Unix-only lock-test import fix. Its Release workflow succeeded and its artifacts are valid; `v0.2.0` has the fix.
- The `v0.4.0` tag's CI check is red on Windows because the new workspace tests compared `std::fs::canonicalize` verbatim paths with the CLI's `dunce` paths. The Release workflow and the Windows artifact smoke passed; the test-only fix is on `main` after the tag.
- The generated release workflow creates the GitHub Release in the `host` phase and then runs the artifact smoke before publishing npm. A broken binary therefore blocks npm publication, but the GitHub Release can already exist; the post-publication `Install smoke` workflow is what verifies the public installer URLs.

## Local checks

```bash
dist generate --check   # release.yml matches dist-workspace.toml
dist plan               # inspect the release plan and artifacts
dist build --target aarch64-apple-darwin --allow-dirty
```

The local build writes artifacts under `target/distrib/`. macOS binaries are unsigned in v1; Homebrew, musl static builds, and code signing are deferred.
