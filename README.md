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
  carrying the out-of-tree drivers (Wi-Fi injection, Bluetooth, CAN, SDR/DVB, NTFS) plus
  the **Picters Manager** WebUI that toggles the injection Wi-Fi stack.

Kernel repo: **[android_kernel_xiaomi_sm8850-nethunter](https://github.com/Picters/android_kernel_xiaomi_sm8850-nethunter)**
— stock kernel on `main`, NetHunter on `resukisu`.

## Layout

- `ci_core_rs/` — the Rust CI core. Parses `configs/projects.json`, runs the kernel build
  (`make`), merges the NetHunter defconfig fragment, builds & packages the out-of-tree
  modules, assembles the OOT-Modules zip (WebUI + `action.sh`) and cuts the release.
- `configs/` — `projects.json` (per-device build metadata) and `anykernel_configs.json`.
- `.github/workflows/` — **Build Kernel**, **Build CI Core**, and helper workflows.

## Build

Dispatch **Build Kernel** from the Actions tab with `project=mi17_sm8850` and
`branch=resukisu`. After editing anything under `ci_core_rs/`, run **Build CI Core** first
and wait for it to finish — the kernel build downloads that prebuilt core, so triggering
both back-to-back would race the rebuild.

## Credits

Base CI & kernel: **Kokuban / YuzakiKokuban** · Root: **ReSukiSU / KernelSU** · SuSFS:
**simonpunk** · Injection drivers: **aircrack-ng**, **morrownr** · NetHunter: **Kali / OffSec**.
