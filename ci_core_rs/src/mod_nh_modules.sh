# Picters OOT module loader library — shared Wi-Fi list + load helpers.
# Sourced by service.sh (boot) and inject.sh (toggle).
KMOD=/system/lib/modules

# The Wi-Fi stack is built against OUR cfg80211 (=m) and conflicts with the vendor
# internal Wi-Fi (qca_cld3 -> vendor cfg80211). It must ONLY be loaded in injection
# mode, so it is EXCLUDED from the boot auto-load. Everything else (CAN, Bluetooth,
# USB-serial, SDR/DVB, USB-ethernet, NTFS) is independent and auto-loads at boot.
NH_WIFI="cfg80211 mac80211 88XXau 8188eu 8814au 88x2bu rtl8xxxu rtlwifi rtl_usb rtl8187 rtl8192cu rtl8192c-common ath ath9k_hw ath9k_common ath9k_htc ath6kl_core ath6kl_usb carl9170 mt7601u rt2x00lib rt2x00usb rt2800lib rt2800usb rt2500usb rt73usb zd1211rw usb_net_rndis_wlan"

nh_is_wifi() { case " $NH_WIFI " in *" $1 "*) return 0 ;; esac; return 1; }

# nh_load <wifi|others> : insmod every matching .ko, several passes so inter-module
# dependencies resolve without needing modules.dep (a failed insmod is skipped).
nh_load() {
  want="$1"; pass=0
  while [ "$pass" -lt 4 ]; do
    pass=$((pass + 1))
    for ko in "$KMOD"/*.ko; do
      [ -f "$ko" ] || continue
      base=${ko##*/}; base=${base%.ko}
      if [ "$want" = wifi ]; then
        nh_is_wifi "$base" || continue
      else
        nh_is_wifi "$base" && continue
      fi
      insmod "$ko" 2>/dev/null
    done
  done
}
