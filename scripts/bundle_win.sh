#!/usr/bin/env bash
# Собирает переносимую папку dist/volna для Windows (запускать в оболочке MSYS2 UCRT64 после cargo build --release).
# Раскладка рядом с exe та же, что ждёт init_windows_environment в main.rs.
set -euo pipefail

P="${MSYSTEM_PREFIX:-/ucrt64}"
OUT=dist/volna

rm -rf dist
mkdir -p "$OUT/lib/gio" "$OUT/libexec/gstreamer-1.0" "$OUT/share/glib-2.0" "$OUT/share/icons"
cp target/release/volna.exe "$OUT/"
cp -r "$P/lib/gstreamer-1.0" "$OUT/lib/"
cp -r "$P/lib/gio/modules" "$OUT/lib/gio/"
cp -r "$P/lib/gdk-pixbuf-2.0" "$OUT/lib/"
cp "$P/libexec/gstreamer-1.0/gst-plugin-scanner.exe" "$OUT/libexec/gstreamer-1.0/"
cp -r "$P/share/glib-2.0/schemas" "$OUT/share/glib-2.0/"
cp -r "$P/share/icons/Adwaita" "$P/share/icons/hicolor" "$OUT/share/icons/"

# DLL-зависимости exe, плагинов и самих скопированных DLL: ещё один проход находит то, что подтянули предыдущие
for pass in 1 2 3; do
    find "$OUT" \( -name '*.exe' -o -name '*.dll' \) -print0 \
        | xargs -0 -n1 ldd 2>/dev/null \
        | awk '/=>/ {print $3}' | grep -F "$P/" | sort -u \
        | while read -r dll; do cp -n "$dll" "$OUT/"; done
done

# без этой проверки неполный комплект проявился бы только на чистой Windows
ls "$OUT"/libgtk-4-1.dll "$OUT"/libadwaita-1-0.dll "$OUT"/libgstreamer-1.0-0.dll > /dev/null
du -sh "$OUT"
