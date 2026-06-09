bash#!/bin/bash

# Настройка путей
SRC="/media/ilja/DATA/prime/"
DST="/media/ilja/DATA/programs/win-prime/prime/"
FILE_TO_CHECK="infra/dockerfile" # Относительный путь к файлу

echo "=== 1. Запуск синхронизации rsync ==="
# Используем -avc (с проверкой по хэшу вместо даты/размера), чтобы dockerfile точно обновился
sudo rsync -avc --progress --exclude 'data' "$SRC" "$DST"

echo -e "\n=== 2. Проверка файла $FILE_TO_CHECK ==="

# Проверяем существование файла в источнике и приемнике
if [ ! -f "$SRC$FILE_TO_CHECK" ]; then
    echo "❌ Ошибка: Файл отсутствует в источнике: $SRC$FILE_TO_CHECK"
    exit 1
fi

if [ ! -f "$DST$FILE_TO_CHECK" ]; then
    echo "❌ Ошибка: Файл отсутствует в приемнике: $DST$FILE_TO_CHECK"
    exit 1
fi

# Получаем MD5 хэши файлов (берем только первую часть вывода md5sum)
SRC_HASH=$(md5sum "$SRC$FILE_TO_CHECK" | awk '{print $1}')
DST_HASH=$(md5sum "$DST$FILE_TO_CHECK" | awk '{print $1}')

echo "Хэш в источнике: $SRC_HASH"
echo "Хэш в приемнике: $DST_HASH"

# Сравниваем хэши
if [ "$SRC_HASH" = "$DST_HASH" ]; then
    echo "✅ Успех! Файлы абсолютно одинаковы."
else
    echo "❌ Внимание! Файлы РАЗЛИЧАЮТСЯ после копирования."
    exit 1
fi
