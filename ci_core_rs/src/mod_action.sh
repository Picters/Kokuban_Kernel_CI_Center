#!/system/bin/sh
# Picters — module Action button (fallback for managers without WebUI): toggles Wi-Fi.
MODDIR=${0%/*}
if grep -q '^qca_cld3_peach_v2 ' /proc/modules; then
  sh "$MODDIR/inject.sh" on
else
  echo "Injection mode is already ON."
  echo "Reboot to switch back to internal Wi-Fi."
fi
