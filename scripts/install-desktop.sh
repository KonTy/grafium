#!/usr/bin/env bash
# Install matching Linux desktop identities and icons without restarting apps.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
bin_dir="$HOME/.local/bin"
assets="$repo_root/ui/src-tauri/icons"
icon_name="grafium-local-icon"
backup="${1:-}"

for asset in 32x32.png 48x48.png 64x64.png 128x128.png 128x128@2x.png grafium-logo.svg; do
  if [[ ! -f "$assets/$asset" ]]; then
    echo "error: missing desktop icon asset: $assets/$asset" >&2
    exit 1
  fi
done

if [[ -z "$backup" ]]; then
  mkdir -p "$HOME/.local/lib/grafium"
  backup="$(mktemp -d "$HOME/.local/lib/grafium/desktop-backup.XXXXXXXX")"
fi
mkdir -p "$backup"
if command -v rebuild-app-cache >/dev/null 2>&1 && [[ -f "$HOME/.cache/smplos/app_index" ]]; then
  cp "$HOME/.cache/smplos/app_index" "$backup/smplos-app_index"
  cmp -s "$HOME/.cache/smplos/app_index" "$backup/smplos-app_index"
fi

install_owned() {
  local source="$1" relative="$2" destination="$data_home/$2" staged
  mkdir -p "$(dirname "$destination")"
  if [[ -e "$destination" || -L "$destination" ]]; then
    mkdir -p "$(dirname "$backup/$relative")"
    cp -L "$destination" "$backup/$relative"
    cmp -s "$destination" "$backup/$relative"
  fi
  staged="$(mktemp "$(dirname "$destination")/.grafium-install.XXXXXXXX")"
  cp "$source" "$staged"
  chmod 644 "$staged"
  mv -fT "$staged" "$destination"
}

# Back a file up, then drop it. Used to retire identities an older install
# created, so the menu keeps exactly one Grafium.
remove_owned() {
  local relative="$1" destination="$data_home/$1"
  if [[ -e "$destination" || -L "$destination" ]]; then
    mkdir -p "$(dirname "$backup/$relative")"
    cp -L "$destination" "$backup/$relative"
    cmp -s "$destination" "$backup/$relative"
    rm -f "$destination"
  fi
}

# Refresh legacy icon aliases too: existing pinned desktop entries may still
# use grafium-local or grafium-bin, and stale rasters take precedence over SVG.
for name in grafium grafium-local grafium-bin "$icon_name"; do
  for size in 32 48 64 128 256; do
    asset="${size}x${size}.png"
    [[ "$size" == 256 ]] && asset="128x128@2x.png"
    install_owned "$assets/$asset" "icons/hicolor/${size}x${size}/apps/$name.png"
  done
  if [[ "$name" != "$icon_name" ]]; then
    install_owned "$assets/grafium-logo.svg" "icons/hicolor/scalable/apps/$name.svg"
  fi
done

# Desktop Exec quoting is not shell quoting.
executable="$bin_dir/grafium"
executable="${executable//\\/\\\\}"
executable="${executable//\"/\\\"}"
executable="${executable//\$/\\\$}"
executable="${executable//\`/\\\`}"
executable="${executable//%/%%}"
# Grafium has exactly one desktop identity. The executable, its WM class and
# this entry all use the `grafium` name, so the menu cannot show a second,
# near-identical launcher pointing at the same app.
desktop="$(mktemp --suffix=.desktop "$backup/.desktop.XXXXXXXX")"
{
  printf '[Desktop Entry]\nVersion=1.0\nType=Application\nName=Grafium\n'
  printf 'Comment=A local-first knowledge graph and note-taking workspace.\n'
  # Some custom menus search system icons before user icons. A distinct local
  # raster name cannot collide with an obsolete packaged grafium.png.
  printf 'Exec="%s"\nIcon=%s\nStartupWMClass=grafium\n' "$executable" "$icon_name"
  # A single main category: listing both Office and Utility makes some menus
  # show Grafium once per section.
  printf 'Terminal=false\nCategories=Office;\nStartupNotify=true\n'
} >"$desktop"
if command -v desktop-file-validate >/dev/null 2>&1; then
  desktop-file-validate "$desktop"
fi
install_owned "$desktop" "applications/grafium.desktop"
rm -- "$desktop"

# Retire the hidden compat entry an older install created for the previous
# `grafium-bin` executable name.
remove_owned "applications/grafium-bin.desktop"

# Preserve pinned legacy desktop IDs and custom fields, but give a legacy
# launcher targeting this same executable the corrected icon as well.
legacy="$data_home/applications/Grafium.desktop"
if [[ -f "$legacy" ]] && grep -Fxq -e "Exec=$bin_dir/grafium" -e "Exec=\"$executable\"" "$legacy"; then
  desktop="$(mktemp --suffix=.desktop "$backup/.legacy.XXXXXXXX")"
  awk -v icon="$icon_name" '
    /^\[/ { in_entry = ($0 == "[Desktop Entry]") }
    in_entry && /^Icon=/ { print "Icon=" icon; next }
    { print }
  ' "$legacy" >"$desktop"
  install_owned "$desktop" "applications/Grafium.desktop"
  rm -- "$desktop"
fi

# smplOS stores pinned launcher commands verbatim. Preserve every pin and its
# order, but normalize this app's old unquoted command to the desktop entry.
pins="$HOME/.config/smplos/pinned-apps.txt"
if [[ -f "$pins" && ! -L "$pins" ]] && grep -Fxq "$bin_dir/grafium" "$pins"; then
  cp "$pins" "$backup/smplos-pinned-apps.txt"
  cmp -s "$pins" "$backup/smplos-pinned-apps.txt"
  staged="$(mktemp "$(dirname "$pins")/.grafium-pins.XXXXXXXX")"
  GRAFIUM_PIN_OLD="$bin_dir/grafium" GRAFIUM_PIN_NEW="\"$executable\"" \
    awk '{ if ($0 == ENVIRON["GRAFIUM_PIN_OLD"]) print ENVIRON["GRAFIUM_PIN_NEW"]; else print }' \
    "$pins" >"$staged"
  chmod --reference="$pins" "$staged"
  mv -fT "$staged" "$pins"
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  if ! gtk-update-icon-cache -f -t "$data_home/icons/hicolor"; then
    echo "warning: icon cache could not be rebuilt; icon files are installed, but the desktop may need to reload them" >&2
  fi
else
  echo "warning: gtk-update-icon-cache is unavailable; the desktop may need an icon refresh" >&2
fi
if command -v update-desktop-database >/dev/null 2>&1; then
  if ! update-desktop-database "$data_home/applications"; then
    echo "warning: desktop metadata cache could not be refreshed; the installed launchers remain available" >&2
  fi
else
  echo "warning: update-desktop-database is unavailable; desktop entries may refresh on next login" >&2
fi
if command -v rebuild-app-cache >/dev/null 2>&1; then
  refreshed=0
  # The installed smplOS watcher can rebuild at the same time and uses a
  # shared temporary filename. Retry this idempotent cache refresh briefly.
  for attempt in 1 2 3; do
    if rebuild-app-cache; then
      refreshed=1
      break
    fi
    [[ "$attempt" == 3 ]] || sleep 0.2
  done
  if [[ "$refreshed" == 0 ]]; then
    echo "warning: smplOS app index could not be refreshed; rerun rebuild-app-cache before reopening its menu" >&2
  fi
fi
echo "installed: Grafium desktop entries and icons (previous files verified in $backup)"
