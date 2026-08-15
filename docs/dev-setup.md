# Windows development setup

## Toolchain

- Install Rust with the official `rustup` installer and use the stable
  `x86_64-pc-windows-msvc` toolchain.
- Install Visual Studio 2022 Desktop development with C++ and a current Windows
  SDK.
- Run `run.bat` for the normal desktop development profile.

For a fast type/error check, prefer:

```powershell
cargo check --manifest-path src-tauri/Cargo.toml --workspace
```

Use a full `cargo build` only when a linked binary is required. For focused DSP
work, prefer:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml -p ministudio-dsp
```

## Antivirus and indexing exclusions

In Windows Security, open **Virus & threat protection → Manage settings →
Exclusions → Add or remove exclusions** and add:

```text
<project>\src-tauri\target
<project>\node_modules
C:\tmp\minidaw-msvc-target
%USERPROFILE%\.cargo
%USERPROFILE%\.rustup
```

`run.bat` uses `C:\tmp\minidaw-msvc-target`, so that is the active Rust cache
on the default Windows workflow. Keep these directories outside OneDrive,
Dropbox, and other cloud-synchronized roots.

In **Indexing Options → Modify**, exclude the project build/cache directories,
especially `target`, `node_modules`, and the external Cargo target directory.

If incremental cache corruption reappears, verify these exclusions first and
then run a one-time `cargo clean`. Do not disable incremental compilation as a
permanent workaround.
