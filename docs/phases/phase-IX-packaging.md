# Phase IX — Packaging

Status: **Implemented; validation passed**

Maps to `project-proposal.md` Batch 8.

## Objective

Produce the Windows-first distributable and a cross-platform smoke-test checklist.

## Deliverables

- Tauri MSI/NSIS bundle configuration.
- Code-signing integration point documented without implementing signing.
- Linux/macOS smoke checklist covering PATH discovery and process-tree termination.
- Clean install, upgrade, launch, and uninstall checks.

## Handoff requirements

Record installer artifacts, target OS/toolchain versions, and unresolved platform-specific risks.

## Implemented scope

- `tauri.windows.conf.json` replaces the base bundle target list with MSI and NSIS only on Windows.
- MSI uses a pinned upgrade code and English (`en-US`) localization; the stable upgrade identity prevents duplicate installed products after future branding changes.
- NSIS uses current-user installation, English localization, LZMA compression, and Tanoor installer/uninstaller icons.
- Both Windows formats block downgrades and use Tauri's WebView2 download bootstrapper.
- Bundle metadata and complete Windows/macOS/Linux icon sources are declared in the shared configuration.
- `npm run bundle:windows` builds both Windows formats; `scripts/verify-windows-bundle.ps1` requires both artifacts and reports size, Authenticode status, and SHA-256.
- `docs/RELEASE_CHECKLIST.md` covers release prerequisites, signing integration, clean installation, upgrade, launch, uninstall, GUI PATH discovery, and process-tree cancellation.

## Signing boundary

No certificate or credentials are committed. The documented integration point is `bundle.windows` in `tauri.windows.conf.json`, using either the native certificate/timestamp fields or a custom `signCommand`. Release verification supports `-RequireSigned` once signing is enabled.

## Validation record

- Host: Windows `10.0.26200` x64, MSVC toolchain, WebView2 `151.0.4129.107`.
- Toolchain: Node `24.10.0`, npm `11.6.1`, Rust/Cargo `1.98.0`, Tauri CLI `2.11.4`, Tauri crate `2.11.5`.
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`: passed.
- `cargo test --manifest-path src-tauri/Cargo.toml --quiet`: 56 passed.
- `npm test`: diff fixtures passed.
- `npm run build`: TypeScript and Vite production build passed.
- `npm run bundle:windows`: built both configured installers.
- `scripts/verify-windows-bundle.ps1`: passed with one MSI and one NSIS artifact.

| Artifact | Bytes | Authenticode | SHA-256 |
|---|---:|---|---|
| `msi/Forge_0.1.0_x64_en-US.msi` | 5,152,768 | Not signed | `29b1a5577861fa7c0342f6384322c0dda4be909cb48327235633097aec338c2c` |
| `nsis/Forge_0.1.0_x64-setup.exe` | 3,565,013 | Not signed | `851d0f7165fb60ba8e5584a12980f127c99079f69dd4b6ee0144f3c1e9cb75fb` |

Artifacts are generated below the ignored `src-tauri/target/release/bundle/` directory and are not committed. Clean VM install/upgrade/uninstall checks and Linux/macOS smoke checks remain release-gate activities in `docs/RELEASE_CHECKLIST.md`; those platforms are not available on this implementation host.
