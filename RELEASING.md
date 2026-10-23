# Releasing Orbit

Orbit uses [cargo-dist](https://axodotdev.github.io/cargo-dist/) for release builds. The configuration lives in `dist-workspace.toml`; `.github/workflows/release.yml` is generated and should not be edited by hand.

## One-time setup

1. Create the `@rcmendes` scope on npm if it does not exist.
2. Create an npm granular access token with read/write access to the scope.
3. Add it to GitHub Actions as the `NPM_TOKEN` repository secret.

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
   - publishes `@rcmendes/orbit` to npm when `NPM_TOKEN` is present.

## Local checks

```bash
dist generate --check   # release.yml matches dist-workspace.toml
dist plan               # inspect the release plan and artifacts
dist build --target aarch64-apple-darwin --allow-dirty
```

The local build writes artifacts under `target/distrib/`. macOS binaries are unsigned in v1; Homebrew, musl static builds, and code signing are deferred.
