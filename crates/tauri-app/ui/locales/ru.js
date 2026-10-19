window.__ZIPTOOL_LOCALES__ ??= {};
window.__ZIPTOOL_LOCALES__["ru"] = {
  "translation": {
    "header": {
      "compress": "Сжать",
      "extract": "Извлечь",
      "confirm": "Подтвердить",
      "cancel": "Отмена",
      "language": "Язык"
    },
    "lang": {
      "system": "Как в системе"
    },
    "path": {
      "src": "Исходная папка",
      "srcPlaceholder": "Нажмите, чтобы выбрать, или перетащите папку для сжатия / извлечения",
      "dst": "Папка назначения",
      "dstPlaceholder": "Нажмите, чтобы выбрать, или перетащите папку назначения",
      "dropRelease": "Отпустите мышь, чтобы выбрать эту папку"
    },
    "options": {
      "split": "Размер части",
      "splitPlaceholder": "Например, 2g / 500m; пусто — без разбиения",
      "password": "Пароль",
      "passwordPlaceholder": "Пусто — без шифрования",
      "randomName": "Случайные имена архивов",
      "verify": "Проверять после сжатия",
      "verifyTitle": "Проверить структуру архива после сжатия и заранее обнаружить неполные архивы",
      "preset": "Режим",
      "presetCompat": "Максимальная совместимость",
      "presetCompatTitle": "Формат zip + максимальное сжатие; открывается в любой системе",
      "presetBalanced": "Баланс",
      "presetBalancedTitle": "Формат 7z (LZMA2), выше степень сжатия; для извлечения нужны инструменты 7z",
      "presetCustom": "Вручную",
      "advanced": "Дополнительно",
      "format": "Формат",
      "level": "Уровень",
      "levelPlaceholder": "По умолчанию",
      "levelTitle": "Пусто = уровень по умолчанию (zip 6 / 7z 5)",
      "exclude": "Исключить",
      "excludePlaceholder": "Через запятую, например Thumbs.db, *.tmp",
      "force": "Перезаписывать существующие файлы",
      "pwHintZip": "Формат zip: шифрование ZipCrypto (лучшая совместимость, слабее)",
      "pwHint7z": "Формат 7z: шифрование AES-256"
    },
    "table": {
      "caption": "Список элементов",
      "selectAll": "Выбрать все",
      "name": "Имя",
      "size": "Размер",
      "status": "Статус",
      "empty": "Нажмите «Исходная папка», чтобы выбрать каталог, или перетащите папку сюда"
    },
    "row": {
      "status": {
        "pendingCompress": "Ожидает сжатия",
        "pendingExtract": "Ожидает извлечения",
        "done": "Готово",
        "running": "Выполняется",
        "failed": "Ошибка",
        "warning": "Предупреждение",
        "meta": "Метаданные"
      },
      "select": "Выбрать {{name}}",
      "original": "Исходное имя: {{name}}"
    },
    "dialog": {
      "selectSourceCompress": "Выберите папку для сжатия",
      "selectSourceExtract": "Выберите папку для извлечения",
      "selectDst": "Выберите папку назначения"
    },
    "status": {
      "ready": "Готово",
      "scanning": "Сканирование {{done}}/{{total}} {{entry}}",
      "scanningStart": "Сканирование…",
      "compressing": "Сжатие {{done}}/{{total}} {{entry}} ({{doneBytes}} / {{totalBytes}})",
      "extracting": "Извлечение {{done}}/{{total}} {{entry}} ({{doneBytes}} / {{totalBytes}})",
      "scanDone_one": "Просканировано {{path}}: {{count}} элемент, всего {{bytes}}",
      "scanDone_few": "Просканировано {{path}}: {{count}} элемента, всего {{bytes}}",
      "scanDone_many": "Просканировано {{path}}: {{count}} элементов, всего {{bytes}}",
      "scanDone_other": "Просканировано {{path}}: {{count}} элемента, всего {{bytes}}",
      "previewScan_one": "Просканировано {{path}}: {{count}} элемент (тестовые данные предпросмотра в браузере)",
      "previewScan_few": "Просканировано {{path}}: {{count}} элемента (тестовые данные предпросмотра в браузере)",
      "previewScan_many": "Просканировано {{path}}: {{count}} элементов (тестовые данные предпросмотра в браузере)",
      "previewScan_other": "Просканировано {{path}}: {{count}} элемента (тестовые данные предпросмотра в браузере)",
      "scanFailed": "Ошибка сканирования: {{error}}",
      "dialogUnavailable": "Плагин выбора папки недоступен; проверьте tauri-plugin-dialog",
      "dialogFailed": "Не удалось открыть окно выбора папки: {{error}}",
      "dstPicked": "Папка назначения: {{path}}",
      "dropBusy": "Выполняется задача; нельзя сменить папку",
      "dropWrongZone": "Перетащите папку в «Исходная папка» или «Папка назначения»",
      "dropOne": "Можно перетащить только одну папку",
      "dropInvalid": "Нельзя использовать перетащенную папку: {{error}}",
      "dragUnavailable": "Перетаскивание папок недоступно: {{error}}",
      "needSelection": "Сначала выберите элементы для обработки",
      "needDst": "Сначала выберите папку назначения",
      "modeSwitched": "Режим изменён; выберите исходную папку заново",
      "cancelled": "Отменено (выполнено {{done}}/{{total}})",
      "compressDone_one": "Сжатие завершено: {{count}} архив → {{dst}}",
      "compressDone_few": "Сжатие завершено: {{count}} архива → {{dst}}",
      "compressDone_many": "Сжатие завершено: {{count}} архивов → {{dst}}",
      "compressDone_other": "Сжатие завершено: {{count}} архива → {{dst}}",
      "extractDone_one": "Извлечение завершено: {{count}} группа → {{dst}}",
      "extractDone_few": "Извлечение завершено: {{count}} группы → {{dst}}",
      "extractDone_many": "Извлечение завершено: {{count}} групп → {{dst}}",
      "extractDone_other": "Извлечение завершено: {{count}} группы → {{dst}}",
      "compressFailed": "Ошибка сжатия: {{error}}",
      "extractFailed": "Ошибка извлечения: {{error}}",
      "warnings_one": "; {{count}} предупреждение: {{first}}",
      "warnings_few": "; {{count}} предупреждения: {{first}}",
      "warnings_many": "; {{count}} предупреждений: {{first}}",
      "warnings_other": "; {{count}} предупреждения: {{first}}"
    },
    "size": {
      "b": "Б",
      "kb": "КБ",
      "mb": "МБ",
      "gb": "ГБ"
    }
  }
};
