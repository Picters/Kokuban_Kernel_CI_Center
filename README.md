# Picters Kernel CI Center

CI/CD center that builds the **Picters NetHunter kernel** for the **Xiaomi 17 Series**
(`sm8850`, codename *pudding*). A Rust core (`ci_core`) drives the whole pipeline — source
sync, toolchain setup, ReSukiSU / SuSFS integration, the kernel build, injection-driver
packaging and GitHub releases — orchestrated by GitHub Actions.

> Fork of the Kokuban CI, pruned and specialised for the Picters NetHunter build.

## What it produces

Every build publishes a release (on the kernel repo) with **two assets**:

- **`Mi17_Kernel-…-ReSuki-…-susfs-….zip`** — the flashable kernel (AnyKernel3).
- **`Mi17_Kernel-…-OOT-Modules-….zip`** — a **manager-agnostic** KernelSU/Magisk module
  carrying the out-of-tree drivers (Wi-Fi injection, Bluetooth, CAN, SDR/DVB, NTFS). All
  drivers stay unloaded on boot; the module's Action button opens the companion
  **Picters Modules Manager** Android app, which insmod/rmmod's each driver on demand over
  root. The APK itself is fetched from the app's own GitHub releases at build time
  (`gh release download --repo Picters/picters_modules_manager`) and staged as a systemless
  `/system/app` install — see `build_oot_module_zip()` in `ci_core_rs/src/build.rs`.

Kernel repo: **[android_kernel_xiaomi_sm8850-nethunter](https://github.com/Picters/android_kernel_xiaomi_sm8850-nethunter)**
— stock kernel on `main`, NetHunter on `resukisu`.
App repo: **[picters_modules_manager](https://github.com/Picters/picters_modules_manager)** —
separate Flutter project, its own release cadence, built independently of the kernel.

## Layout

- `ci_core_rs/` — the Rust CI core. Parses `configs/projects.json`, runs the kernel build
  (`make`), merges the NetHunter defconfig fragment, builds & packages the out-of-tree
  modules, assembles the OOT-Modules zip (`action.sh` opens the manager app) and cuts
  the release.
- `configs/` — `projects.json` (per-device build metadata) and `anykernel_configs.json`.
- `.github/workflows/` — **Build Kernel**, **Build CI Core**, and helper workflows.

## Build

Dispatch **Build Kernel** from the Actions tab with `project=mi17_sm8850` and
`branch=resukisu`. After editing anything under `ci_core_rs/`, run **Build CI Core** first
and wait for it to finish — the kernel build downloads that prebuilt core, so triggering
both back-to-back would race the rebuild.

## Known issues

The Wi-Fi injection drivers (aircrack-ng `rtl8812au`/`rtl8188eus`, morrownr `8814au`/`88x2bu`)
are old vendor codebases never written against a CFI- and UBSAN-hardened GKI kernel. We've
already found and patched several classes of runtime-only bugs this exposes (mismatched
callback prototypes under `CONFIG_CFI_CLANG`, fixed-size arrays that trip UBSAN's
array-bounds sanitizer — see `patch_realtek_cfi()` / `patch_realtek_ubsan()` in
[`ci_core_rs/src/build.rs`](ci_core_rs/src/build.rs)), but more may exist in code paths we
haven't exercised yet.

**If an adapter crashes or hard-reboots the device, please open an issue** on this repo with
whatever you were doing at the time and, if possible, the panic trace from
`/data/vendor/diag/last_kmsg` (survives the reboot; `su -c 'cat /data/vendor/diag/last_kmsg'`).
That trace is normally enough to pin down the exact function and fix it.

## Credits

Base CI & kernel: **Kokuban / YuzakiKokuban** · Root: **ReSukiSU / KernelSU** · SuSFS:
**simonpunk** · Injection drivers: **aircrack-ng**, **morrownr** · NetHunter: **Kali / OffSec**.
