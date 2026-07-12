#!/system/bin/sh
# Picters OOT modules — boot auto-load (KernelSU/Magisk late_start service).
# Loads the non-Wi-Fi drivers (CAN, Bluetooth, USB-serial, SDR/DVB, USB-ethernet,
# NTFS) so plugging a USB adapter or mounting NTFS just works — no manager needed.
# The Wi-Fi injection stack is intentionally NOT loaded here (it needs the toggle).
MODDIR=${0%/*}
. "$MODDIR/nh-modules.sh"

# Give the module overlay a moment to be mounted onto /system after boot.
i=0
while [ ! -e "$KMOD/ntfs3.ko" ] && [ "$i" -lt 20 ]; do i=$((i + 1)); sleep 2; done

nh_load others
