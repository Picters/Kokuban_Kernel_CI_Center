# Picters Kernel CI Center

CI/CD that builds the **Picters kernel** (with extra out-of-tree modules) and the **Modules pack**
for the **Xiaomi 17 Series** (`sm8850`, *pudding*) — a Rust core (`ci_core`) driving source sync,
toolchain, ReSukiSU/SuSFS, the build and packaging via GitHub Actions.

Each release ships two assets: the flashable kernel (AnyKernel3) and a manager-agnostic
KernelSU/Magisk **Modules pack** (Wi-Fi injection, BT, CAN, SDR/DVB, NTFS). Non-Wi-Fi drivers load
at boot; Wi-Fi injection stays off until switched on in the Picters Modules Manager app.

## Build

Dispatch **Build Kernel** (`project=mi17_sm8850`, `branch=resukisu`) from the Actions tab. After
editing anything under `ci_core_rs/`, run **Build CI Core** first and let it finish.

## Credits

Base CI & kernel: **Kokuban / YuzakiKokuban** · Root: **ReSukiSU / KernelSU** · SuSFS:
**simonpunk** · Injection drivers: **aircrack-ng**, **morrownr**.
