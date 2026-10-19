window.__ZIPTOOL_LOCALES__ ??= {};
window.__ZIPTOOL_LOCALES__["zh-TW"] = {
  "translation": {
    "header": {
      "compress": "壓縮",
      "extract": "解壓",
      "confirm": "確認",
      "cancel": "取消",
      "language": "語言"
    },
    "lang": {
      "system": "跟隨系統"
    },
    "path": {
      "src": "來源資料夾",
      "srcPlaceholder": "點選以選擇, 或拖入要壓縮 / 解壓的資料夾",
      "dst": "目標資料夾",
      "dstPlaceholder": "點選以選擇, 或拖入目標資料夾",
      "dropRelease": "放開滑鼠以選取此資料夾"
    },
    "options": {
      "split": "分卷",
      "splitPlaceholder": "如 2g / 500m, 留空不分卷",
      "password": "密碼",
      "passwordPlaceholder": "留空不加密",
      "randomName": "隨機壓縮檔名",
      "verify": "壓縮成功後驗證",
      "verifyTitle": "壓縮完成後檢查壓縮檔結構, 及早發現不完整的壓縮檔",
      "preset": "模式",
      "presetCompat": "最大相容性",
      "presetCompatTitle": "zip 格式 + 最大壓縮強度; 所有系統都能開啟",
      "presetBalanced": "平衡",
      "presetBalancedTitle": "7z 格式(LZMA2), 壓縮率更高; 需 7z 系工具解壓",
      "presetCustom": "自訂",
      "advanced": "進階",
      "format": "格式",
      "level": "強度",
      "levelPlaceholder": "預設",
      "levelTitle": "留空 = 後端預設強度(zip 6 / 7z 5)",
      "exclude": "排除",
      "excludePlaceholder": "以逗號分隔, 如 Thumbs.db, *.tmp",
      "force": "覆寫現有檔案",
      "pwHintZip": "zip 格式: ZipCrypto 加密(相容性最好, 強度較弱)",
      "pwHint7z": "7z 格式: AES-256 加密"
    },
    "table": {
      "caption": "項目列表",
      "selectAll": "全選",
      "name": "檔案名稱",
      "size": "大小",
      "status": "狀態",
      "empty": "點選「來源資料夾」選擇目錄, 也可直接拖入資料夾"
    },
    "row": {
      "status": {
        "pendingCompress": "待壓縮",
        "pendingExtract": "待解壓",
        "done": "完成",
        "running": "進行中",
        "failed": "失敗",
        "warning": "警告",
        "meta": "中繼資料"
      },
      "select": "選取 {{name}}",
      "original": "原名 {{name}}"
    },
    "dialog": {
      "selectSourceCompress": "選取要壓縮的資料夾",
      "selectSourceExtract": "選取要解壓的資料夾",
      "selectDst": "選取目標資料夾"
    },
    "status": {
      "ready": "就緒",
      "scanning": "掃描中 {{done}}/{{total}} {{entry}}",
      "scanningStart": "掃描中…",
      "compressing": "壓縮中 {{done}}/{{total}} {{entry}} ({{doneBytes}} / {{totalBytes}})",
      "extracting": "解壓中 {{done}}/{{total}} {{entry}} ({{doneBytes}} / {{totalBytes}})",
      "scanDone_other": "已掃描 {{path}}: {{count}} 個項目, 共 {{bytes}}",
      "previewScan_other": "已掃描 {{path}}: {{count}} 個項目(瀏覽器預覽假資料)",
      "scanFailed": "掃描失敗: {{error}}",
      "dialogUnavailable": "資料夾選擇外掛無法使用, 請檢查 tauri-plugin-dialog",
      "dialogFailed": "無法開啟資料夾選擇視窗: {{error}}",
      "dstPicked": "目標資料夾: {{path}}",
      "dropBusy": "任務進行中, 無法更換資料夾",
      "dropWrongZone": "請將資料夾拖到「來源資料夾」或「目標資料夾」",
      "dropOne": "一次只能拖入一個資料夾",
      "dropInvalid": "無法使用拖入的資料夾: {{error}}",
      "dragUnavailable": "資料夾拖曳無法使用: {{error}}",
      "needSelection": "請先勾選要處理的項目",
      "needDst": "請先選取目標資料夾",
      "modeSwitched": "模式已切換, 請重新選取來源資料夾",
      "cancelled": "已取消(完成 {{done}}/{{total}})",
      "compressDone_other": "壓縮完成: {{count}} 個壓縮檔 → {{dst}}",
      "extractDone_other": "解壓完成: {{count}} 組 → {{dst}}",
      "compressFailed": "壓縮失敗: {{error}}",
      "extractFailed": "解壓失敗: {{error}}",
      "warnings_other": "; {{count}} 則警告: {{first}}"
    },
    "size": {
      "b": "B",
      "kb": "KB",
      "mb": "MB",
      "gb": "GB"
    }
  }
};
