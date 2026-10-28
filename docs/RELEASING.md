# Releasing Orbit

Orbit uses [cargo-dist](https://axodotdev.github.io/cargo-dist/) for release builds. The configuration lives in `dist-workspace.toml`; `.github/workflows/release.yml` is generated and should not be edited by hand.

## Released versions

- `v0.9.0` — latest
- `v0.8.2`
- `v0.8.1`
- `v0.8.0` — deprecated (Windows CI failure; npm only)
- `v0.7.0`
- `v0.6.0`
- `v0.5.0`
- `v0.4.0`
- `v0.3.0`
- `v0.2.0`
- `v0.1.0` — initial release

## One-time setup

1. Ensure the `@rcmendes` npm scope exists and is owned by the publishing account. No organization is needed when `rcmendes` is the npm username.
2. Create an npm granular access token with read/write access to `@rcmendes`.
3. Enable **Bypass 2FA** on the token. Without it, `npm publish` fails with `E403` even when the token has publish permission. npm also rejects `npm unpublish` with a bypass-2FA granular token (`E403`), so a bad version must be superseded by a new version or unpublished interactively with an OTP.
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

1. Update `version` in `Cargo.toml` and add a matching heading to `../CHANGELOG.md` (for example `## 0.9.0 - 2026-10-28`). The changelog must stay at the repository root: cargo-dist only discovers release-note entries there.
2. Commit the changes.
3. Tag and push:

   ```bash
   git tag v0.9.0
   git push origin v0.9.0
   ```

4. The `Release` workflow:
   - plans and builds macOS arm64/x86_64, Linux x86_64/arm64, and Windows x86_64;
   - produces `.tar.xz`/`.zip` archives, shell and PowerShell installers, checksums, and the npm package;
   - creates the GitHub Release with changelog notes;
   - publishes `@rcmendes/orbit` to npm using `NPM_TOKEN`.

5. Trigger the post-release smoke check immediately after the Release workflow finishes:

   ```bash
   gh workflow run "Install smoke" --ref v0.9.0
   ```

    A release created by `GITHUB_TOKEN` does not emit a downstream workflow event, so `Install smoke` does not start automatically. It installs the latest public shell/PowerShell installers and the npm package, then exercises `orbit --version`, onboarding, `orbit github`/`orbit triage` help, and a fixture-backed GitHub flow (connect, status, adopt/create, sync preview/apply, and Triage review/apply) without network access, plus a real MCP initialize/tools-list handshake. A red run means the published artifacts are broken even though the release exists.

## Packaging checks

`scripts/onboarding-smoke.sh` and `scripts/onboarding-smoke.ps1` are the single onboarding sequence used by CI and available locally:

```bash
cargo build --release
ORBIT_BIN="$PWD/target/release/orbit" bash scripts/onboarding-smoke.sh
```

Both scripts run in a temporary Git repository with a temporary `HOME`/`USERPROFILE`, create a project-local `.claude` marker so at least Claude is always detected, and never read or write the developer's real agent configuration. They must stay in sync with the documented quick start; do not add a second smoke sequence. GitHub operations use `ORBIT_GITHUB_FIXTURE`; never remove that fixture or let the release smoke contact GitHub.

The release defaults to the Cargo `github-provider` feature. Keep that feature
in `[features].default` so cargo-dist archives, installers, and the binary
bundled by npm all include the provider. CI also checks the public CLI help in
the archived binaries. A build with `--no-default-features` may omit the
provider; it must fail clearly when it loads an enabled `[github]` Board and
direct users to Orbit v0.9.0 or newer. Do not bump the package version as part
of feature integration; the release owner chooses the tag/version when cutting
the release.

## npm notes

- The first publish of a new package can briefly create a `0.0.0-stage` placeholder while npm processes the upload. It is not the `latest` tag; the real version appears after processing.
- Push tags one at a time, oldest first, so npm's `latest` tag ends on the newest version.
- A published npm version is immutable. Fix a bad release by publishing a new version, not by moving its tag.
- npm rejects unpublish with a granular token that bypasses 2FA (`E403`). Use a new version, or unpublish interactively with an OTP within npm's 72-hour window.

## Known issues

- `v0.8.0` failed Windows CI because the doctor tests used Unix-only absolute paths and `git worktree move` could not move a clean worktree out of `.git`. Its GitHub release and tag were removed. The npm package remains published and deprecated in favor of `v0.8.1`; the bypass-2FA token cannot unpublish it, so fix a bad version by publishing a new one rather than trying to remove it by tag.
- The `v0.7.0` tag's CI check is red on Windows because `tests/slug_migrate.rs` asserted an LF-only YAML dependency line; Git checked the migrated file out with CRLF. The Release workflow and its Windows artifact smoke passed, and npm published successfully. The test-only fix is on `main` after the tag.
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
