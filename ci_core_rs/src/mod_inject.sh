#!/system/bin/sh
# Picters Wi-Fi injection toggle.  Usage: inject.sh on | off
# on  -> unload vendor Wi-Fi, load kernel cfg80211 + ALL injection adapters
# off -> internal Wi-Fi can only be restored by a reboot (Qualcomm firmware)
MODDIR=${0%/*}
. "$MODDIR/nh-modules.sh"
VMOD=/vendor_dlkm/lib/modules

case "$1" in
  on)
    if grep -q '^qca_cld3_peach_v2 ' /proc/modules; then
      echo "Disabling internal Wi-Fi..."
      svc wifi disable 2>/dev/null
      sleep 2
      rmmod qca_cld3_peach_v2 2>/dev/null
    fi
    rmmod mac80211 2>/dev/null
    rmmod cfg80211 2>/dev/null
    echo "Loading kernel Wi-Fi stack + injection adapters..."
    nh_load wifi
    if grep -q '^cfg80211 ' /proc/modules; then
      echo "Injection mode ON. Plug any supported adapter, then run: iw dev"
    else
      echo "Failed to load kernel cfg80211 — reboot and try again."
    fi
    ;;
  off)
    echo "Internal Wi-Fi uses the Qualcomm firmware and can only be re-initialised"
    echo "by a reboot. Reboot to switch back to internal Wi-Fi."
    ;;
  *)
    echo "usage: inject.sh on|off"
    ;;
esac
