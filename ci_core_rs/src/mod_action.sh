#!/system/bin/sh
# Picters — module Action button: opens the Picters Modules Manager app.
# The app (not this script) does the insmod/rmmod work now — see
# picters_modules_manager/. No LAUNCHER icon on the app, so this is the
# only way to open it besides `am start` over adb.
am start -n com.picters.modulesmanager/.MainActivity >/dev/null 2>&1
if [ $? -ne 0 ]; then
  echo "Picters Modules Manager is not installed."
  echo "Install its APK, then use this Action button again."
fi
