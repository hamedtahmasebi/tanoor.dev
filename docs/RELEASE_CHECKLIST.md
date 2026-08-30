# Tanoor release and platform smoke checklist

Use this checklist for every distributable release. Windows is the production packaging target for v1; Linux and macOS remain smoke-tested secondary targets.

## Release inputs

- [ ] Set the same SemVer in `package.json`, `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`.
- [ ] Review all intentional working-tree changes and run `git diff --check`.
- [ ] Run `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`.
- [ ] Run `cargo test --manifest-path src-tauri/Cargo.toml`.
- [ ] Run `npm test` and `npm run build`.
- [ ] Confirm the release host has the Tauri prerequisites, Node.js, Rust stable, and the `x86_64-pc-windows-msvc` target.
- [ ] Confirm Windows Script Host/VBScript is enabled before building MSI packages.

## Windows bundle

Build both installer formats on Windows:

```powershell
npm ci
npm run bundle:windows
powershell -ExecutionPolicy Bypass -File scripts/verify-windows-bundle.ps1
```

Expected output locations:

- MSI: `src-tauri/target/release/bundle/msi/*.msi`
- NSIS: `src-tauri/target/release/bundle/nsis/*-setup.exe`

The Windows platform override pins MSI upgrade code `8aa41bb8-5fd1-53bc-9824-773847b416c6`. Never change it for the same product line. NSIS installs per-user without elevation, downgrades are blocked, and both installers use the small WebView2 download bootstrapper. A clean installation therefore needs internet access when WebView2 is absent.

### Signing integration point

Signing is intentionally not enabled in source control. A release owner can add certificate-backed fields under `bundle.windows` in `src-tauri/tauri.windows.conf.json`:

```json
{
  "digestAlgorithm": "sha256",
  "certificateThumbprint": "<SHA-1 certificate thumbprint>",
  "timestampUrl": "<certificate-provider timestamp URL>",
  "tsp": false
}
```

For a hardware token, cloud key vault, or non-Windows signing host, use `bundle.windows.signCommand` instead. Its command must accept `%1` as the artifact-path placeholder. Keep credentials outside the repository and inject them through the release environment. After signing is configured, require valid Authenticode signatures:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/verify-windows-bundle.ps1 -RequireSigned
```

## Windows install, upgrade, and uninstall

Run MSI and NSIS checks separately on clean Windows 10 and Windows 11 VMs. Uninstall one format before testing the other.

- [ ] Verify publisher, product name, version, icon, destination, and start-menu entry in the installer UI.
- [ ] Complete a clean install as a standard user; NSIS must not request elevation.
- [ ] Launch Tanoor from the Start menu rather than a developer terminal.
- [ ] Open Settings and confirm Git and Codex versions plus Codex login status are detected.
- [ ] If GUI PATH discovery fails, set absolute Git/Codex paths, save without restarting, and recheck health.
- [ ] Add a Git repository, create a task, run it, review its diff, and confirm it.
- [ ] Start a task long enough to cancel it. After cancellation, confirm no descendant `codex` process remains with Task Manager or `Get-CimInstance Win32_Process`.
- [ ] Install a higher Tanoor version over the existing version. Confirm there is only one installed-app entry and the project/task database remains intact.
- [ ] Attempt to install the older version and confirm the downgrade is rejected.
- [ ] Uninstall Tanoor and confirm its executable, shortcuts, and installed-app entry are removed.
- [ ] Confirm user data remains recoverable under the app-data directory; v1 uninstallers intentionally do not delete the SQLite database or task logs.

## Linux smoke test

Build on a supported Linux host with the required WebKitGTK and bundler packages:

```bash
npm ci
npm run bundle
```

- [ ] Install and launch the native bundle from the desktop environment, not a shell.
- [ ] In Settings, verify GUI-launched PATH discovery for `git` and `codex`; test absolute-path overrides if the desktop PATH differs from the login shell.
- [ ] Verify Codex login status, create a worktree-backed task, complete a turn, and review the diff.
- [ ] Cancel a running turn, then use `pgrep -af codex` to confirm the Codex process group and descendants are gone.
- [ ] Confirm the original repository has no unexpected checked-out files and the task worktree remains available for cancelled-task recovery.
- [ ] Upgrade to the next bundle and verify the SQLite data persists; uninstall and verify application files are removed while user data is retained.
- [ ] Record distribution/version, desktop environment, display server, package format, Git/Codex versions, and result below.

## macOS smoke test

Build on both architecture families that will be distributed, or explicitly record the architecture not tested:

```bash
npm ci
npm run bundle
```

- [ ] Install the `.app`/DMG and launch from Finder, not Terminal.
- [ ] In Settings, verify GUI-launched PATH discovery for `git` and `codex`; Homebrew paths commonly require an absolute override when Finder's PATH is minimal.
- [ ] Verify Codex login status, create a worktree-backed task, complete a turn, and review the diff.
- [ ] Cancel a running turn, then use `pgrep -af codex` to confirm the Codex process group and descendants are gone.
- [ ] Confirm the original repository has no unexpected checked-out files and the task worktree remains available for cancelled-task recovery.
- [ ] Upgrade to the next bundle and verify the SQLite data persists; remove the application and verify user data remains recoverable.
- [ ] Record macOS version, Intel/Apple Silicon architecture, Git/Codex versions, signing/notarization state, and result below.

## Release record

Copy this table into the release notes and fill every row. Attach the Windows verifier output so artifact names, byte sizes, signatures, and SHA-256 hashes are preserved.

| Target | OS/toolchain | Artifact or package | Install | PATH/auth | Run/review | Cancel tree | Upgrade | Uninstall | Notes |
|---|---|---|---|---|---|---|---|---|---|
| Windows 10 x64 | | MSI | | | | | | | |
| Windows 10 x64 | | NSIS | | | | | | | |
| Windows 11 x64 | | MSI | | | | | | | |
| Windows 11 x64 | | NSIS | | | | | | | |
| Linux x64 | | Native bundle | | | | | | | |
| macOS Apple Silicon | | App/DMG | | | | | | | |
| macOS Intel | | App/DMG | | | | | | | |

## Known v1 release risks

- Windows process-tree termination uses `taskkill /F /T`; it must be exercised against a real packaged Codex child process before release.
- Linux/macOS termination relies on starting Codex in its own process group and killing that group; verify no grandchildren survive on each target OS.
- GUI applications can inherit a smaller PATH than interactive shells. Absolute binary overrides are the supported recovery path.
- Installers do not bundle Git or Codex and do not delete Tanoor user data during uninstall.
- Windows artifacts are unsigned until the signing integration point above is configured; SmartScreen warnings are expected for downloaded unsigned builds.
