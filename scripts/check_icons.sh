#!/usr/bin/env bash
# Скрипт аудита иконок для приложения «Волна»
# Проверяет, что все иконки, вызываемые в коде src/, присутствуют либо в системной теме Adwaita,
# либо во встроенных ресурсах приложения (data/resources.gresource.xml).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

SEARCH_DIR="${1:-}"

if [ -z "$SEARCH_DIR" ]; then
    if [ -n "${MSYSTEM_PREFIX:-}" ] && [ -d "${MSYSTEM_PREFIX}/share/icons/Adwaita" ]; then
        SEARCH_DIR="${MSYSTEM_PREFIX}/share/icons/Adwaita"
    elif [ -d "/ucrt64/share/icons/Adwaita" ]; then
        SEARCH_DIR="/ucrt64/share/icons/Adwaita"
    elif [ -d "/usr/share/icons/Adwaita" ]; then
        SEARCH_DIR="/usr/share/icons/Adwaita"
    else
        echo "[ERROR] Каталог темы Adwaita не найден ни по одному стандартному пути!"
        exit 1
    fi
fi

GRESOURCE_XML="$REPO_ROOT/data/resources.gresource.xml"

echo "================================================================="
echo "Аудит иконок приложения «Волна»"
echo "Каталог темы Adwaita: $SEARCH_DIR"
echo "Манифест GResource:  $GRESOURCE_XML"
echo "================================================================="

# Динамическое извлечение имён иконок из исходного кода src/
mapfile -t ICONS < <(
    {
        grep -roh -E '(from_icon_name|set_icon_name)\s*\(\s*(Some\()?\s*"[^"]+"' "$REPO_ROOT/src" | sed -E 's/.*"([^"]+)".*/\1/'
        grep -roh -E 'icon_name\s*=\s*(if[^{]+)?\s*"[^"]+"' "$REPO_ROOT/src" | sed -E 's/.*"([^"]+)".*/\1/'
        grep -roh -E '"[a-zA-Z0-9_-]+-symbolic"' "$REPO_ROOT/src" | tr -d '"'
    } | sort -u
)

if [ "${#ICONS[@]}" -eq 0 ]; then
    echo "[ERROR] Не удалось извлечь ни одной иконки из $REPO_ROOT/src!"
    exit 1
fi

FOUND_COUNT=0
MISSING_COUNT=0
MISSING_LIST=()

printf "%-35s | %-18s | %s\n" "Имя иконки" "Статус" "Источник / Путь"
echo "--------------------------------------------------------------------------------------------------"

for icon in "${ICONS[@]}"; do
    FILE_PATH=$(find "$SEARCH_DIR" \( -name "${icon}.svg" -o -name "${icon}.png" \) 2>/dev/null | head -n 1 || true)
    if [ -n "$FILE_PATH" ]; then
        printf "%-35s | %-18s | %s\n" "$icon" "FOUND (Adwaita)" "$FILE_PATH"
        FOUND_COUNT=$((FOUND_COUNT + 1))
    elif [ -f "$GRESOURCE_XML" ] && grep -q "${icon}.svg" "$GRESOURCE_XML"; then
        printf "%-35s | %-18s | %s\n" "$icon" "FOUND (GResource)" "data/resources.gresource.xml"
        FOUND_COUNT=$((FOUND_COUNT + 1))
    else
        printf "%-35s | %-18s | %s\n" "$icon" "MISSING" "-"
        MISSING_COUNT=$((MISSING_COUNT + 1))
        MISSING_LIST+=("$icon")
    fi
done

echo "================================================================="
echo "Итог аудита: Найдено: $FOUND_COUNT / ${#ICONS[@]}, Отсутствует: $MISSING_COUNT"
if [ "$MISSING_COUNT" -gt 0 ]; then
    echo "[ERROR] Отсутствующие иконки, не найденные ни в Adwaita, ни в GResource:"
    for m in "${MISSING_LIST[@]}"; do
        echo "  - $m"
    done
    echo "================================================================="
    exit 1
fi

echo "[SUCCESS] Все иконки гарантированно доступны для отображения!"
echo "================================================================="
exit 0
