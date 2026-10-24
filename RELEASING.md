# Releasing Orbit

Orbit uses [cargo-dist](https://axodotdev.github.io/cargo-dist/) for release builds. The configuration lives in `dist-workspace.toml`; `.github/workflows/release.yml` is generated and should not be edited by hand.

## Released versions

- `v0.2.0` — latest
- `v0.1.0` — initial release

## One-time setup

1. Ensure the `@rcmendes` npm scope exists and is owned by the publishing account. No organization is needed when `rcmendes` is the npm username.
2. Create an npm granular access token with read/write access to `@rcmendes`.
3. Enable **Bypass 2FA** on the token. Without it, `npm publish` fails with `E403` even when the token has publish permission.
4. Add the token to GitHub Actions as the `NPM_TOKEN` repository secret:

   ```bash
   gh secret set NPM_TOKEN -R rcmendes/orbit
   ```

## Cut a release

1. Update `version` in `Cargo.toml` and add a matching heading to `CHANGELOG.md` (for example `## 0.2.0 - 2026-11-01`).
2. Commit the changes.
3. Tag and push:

   ```bash
   git tag v0.2.0
   git push origin v0.2.0
   ```

4. The `Release` workflow:
   - plans and builds macOS arm64/x86_64, Linux x86_64/arm64, and Windows x86_64;
   - produces `.tar.xz`/`.zip` archives, shell and PowerShell installers, checksums, and the npm package;
   - creates the GitHub Release with changelog notes;
   - publishes `@rcmendes/orbit` to npm using `NPM_TOKEN`.

## npm notes

- The first publish of a new package can briefly create a `0.0.0-stage` placeholder while npm processes the upload. It is not the `latest` tag; the real version appears after processing.
- Push tags one at a time, oldest first, so npm's `latest` tag ends on the newest version.
- A published npm version is immutable. Fix a bad release by publishing a new version, not by moving its tag.

## Known issues

- The `v0.1.0` tag's CI check is red on Windows because it predates the Unix-only lock-test import fix. Its Release workflow succeeded and its artifacts are valid; `v0.2.0` has the fix.

## Local checks

```bash
dist generate --check   # release.yml matches dist-workspace.toml
dist plan               # inspect the release plan and artifacts
dist build --target aarch64-apple-darwin --allow-dirty
```

The local build writes artifacts under `target/distrib/`. macOS binaries are unsigned in v1; Homebrew, musl static builds, and code signing are deferred.
