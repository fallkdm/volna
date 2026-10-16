#!/usr/bin/env bash
# Скрипт аудита иконок Adwaita для приложения «Волна»
set -u

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

echo "================================================================="
echo "Аудит иконок в каталоге темы: $SEARCH_DIR"
echo "================================================================="

ICONS=(
    "action-unavailable-symbolic"
    "audio-volume-high-symbolic"
    "audio-x-generic-symbolic"
    "avatar-default-symbolic"
    "edit-paste-symbolic"
    "emblem-favorite-symbolic"
    "emblem-system-symbolic"
    "folder-music-symbolic"
    "go-next-symbolic"
    "go-previous-symbolic"
    "media-playback-pause-symbolic"
    "media-playback-start-symbolic"
    "media-playlist-repeat-symbolic"
    "media-skip-backward-symbolic"
    "media-skip-forward-symbolic"
    "pan-down-symbolic"
    "preferences-system-symbolic"
    "system-search-symbolic"
    "text-x-generic-symbolic"
    "window-close-symbolic"
)

FOUND_COUNT=0
MISSING_COUNT=0
MISSING_LIST=()

printf "%-35s | %-10s | %s\n" "Имя иконки" "Статус" "Путь к файлу"
echo "--------------------------------------------------------------------------------------------------"

for icon in "${ICONS[@]}"; do
    FILE_PATH=$(find "$SEARCH_DIR" \( -name "${icon}.svg" -o -name "${icon}.png" \) 2>/dev/null | head -n 1)
    if [ -n "$FILE_PATH" ]; then
        printf "%-35s | %-10s | %s\n" "$icon" "FOUND" "$FILE_PATH"
        FOUND_COUNT=$((FOUND_COUNT + 1))
    else
        printf "%-35s | %-10s | %s\n" "$icon" "MISSING" "-"
        MISSING_COUNT=$((MISSING_COUNT + 1))
        MISSING_LIST+=("$icon")
    fi
done

echo "================================================================="
echo "Итог аудита: Найдено: $FOUND_COUNT / ${#ICONS[@]}, Отсутствует: $MISSING_COUNT"
if [ "$MISSING_COUNT" -gt 0 ]; then
    echo "Отсутствующие в Adwaita иконки:"
    for m in "${MISSING_LIST[@]}"; do
        echo "  - $m"
    done
fi
echo "================================================================="

# Скрипт не завершается с ошибкой при отсутствующих иконках, чтобы CI зафиксировал аудит в логе
exit 0
