window.__ZIPTOOL_LOCALES__ ??= {};
window.__ZIPTOOL_LOCALES__["zh-HK"] = {
  "translation": {
    "header": {
      "compress": "壓縮",
      "extract": "解壓",
      "confirm": "確認",
      "cancel": "取消",
      "language": "語言",
      "scanning": "掃描中…"
    },
    "lang": {
      "system": "跟隨系統"
    },
    "menu": {
      "label": "應用程式選單",
      "app": "應用",
      "reportIssue": "回報問題",
      "quit": "結束",
      "settings": "設定",
      "openSettings": "設定…"
    },
    "path": {
      "src": "來源資料夾",
      "srcPlaceholder": "點擊選擇, 或拖入要壓縮 / 解壓的資料夾",
      "dst": "目標資料夾",
      "dstPlaceholder": "點擊選擇, 或拖入目標資料夾",
      "dropRelease": "放開滑鼠以選擇此資料夾"
    },
    "options": {
      "split": "分卷",
      "splitPlaceholder": "如 2g / 500m, 留空不分卷",
      "password": "密碼",
      "passwordPlaceholder": "留空不加密",
      "randomName": "隨機壓縮檔名",
      "verify": "壓縮成功後驗證",
      "verifyTitle": "壓縮完成後檢查壓縮檔結構, 提前發現不完整的壓縮檔",
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
      "force": "覆蓋現有檔案",
      "smartPackaging": "智能打包",
      "smartPackagingTitle": "勾選後按打包策略自動合併小檔案並分包; 不勾選則每個頂層條目單獨壓縮",
      "pwHintZip": "zip 格式: ZipCrypto 加密(相容性最好, 強度較弱)",
      "pwHint7z": "7z 格式: AES-256 加密"
    },
    "strategy": {
      "label": "打包策略",
      "mergeBelow": "合併≤",
      "mergeBelowPlaceholder": "100m",
      "mergeBelowTitle": "小於等於此大小的頂層項目會進入合併池; 留空不合併",
      "splitAbove": "分卷>",
      "splitAbovePlaceholder": "2g",
      "splitAboveTitle": "大於此大小的項目會按此大小分卷; 留空不分卷",
      "bundleMax": "合併包≤",
      "bundleMaxPlaceholder": "跟隨分卷",
      "bundleMaxTitle": "單一合併包的原始大小上限; 留空跟隨分卷門檻",
      "maxEntries": "項目≤",
      "maxEntriesPlaceholder": "50000",
      "maxEntriesTitle": "單一合併包的項目數上限; 留空不限制"
    },
    "plan": {
      "strategy": { "direct": "壓縮", "split": "分卷", "bundle": "合併" },
      "group": {
        "bundle": "合併打包 #{{n}}",
        "count": "共 {{count}} 個檔案",
        "unselected": "未納入打包",
        "meta": "{{members}} 項 · {{entries}} 個項目",
        "unselectedTip": "{{count}} 個項目未勾選, 勾選後會重新計算方案"
      },
      "reason": {
        "split": "{{size}} > 分卷門檻 {{limit}}",
        "merge": "{{size}} ≤ 合併門檻 {{limit}}",
        "direct": "{{size}} 未進入合併池"
      },
      "summary": {
        "idle": "選擇來源資料夾後產生打包方案",
        "noSelection": "未選擇項目",
        "planning": "正在計算打包方案…",
        "error": "方案無效: {{error}}",
        "text": "預計 {{total}} 個壓縮包（{{direct}} 壓縮 · {{split}} 分卷 · {{bundle}} 合併）· 最大 {{max}}",
        "splitNote": "{{count}} 個分卷按 {{limit}} 切卷",
        "warnings": "{{count}} 個提醒"
      },
      "warning": {
        "tooManyEntries": "{{name}} 項目數 {{count}} 超過上限 {{limit}}, 已改為直接壓縮"
      }
    },
    "settings": {
      "title": "設定",
      "back": "返回",
      "nav": {
        "label": "設定導覽",
        "general": "一般",
        "filters": "檔案排除"
      },
      "general": {
        "heading": "一般",
        "shutdown": {
          "title": "壓縮完成後關機",
          "desc": "即將支援: 壓縮成功後自動關機的功能將於後續版本提供。"
        }
      },
      "filters": {
        "heading": "檔案排除",
        "intro": "策略在壓縮掃描時生效; 修改後會在關閉設定頁時重新掃描目前來源資料夾。被排除的項目仍會顯示, 但預設不勾選。單次任務仍可在主介面的「進階」中加入自訂模式。",
        "osHidden": {
          "title": "作業系統隱藏檔案",
          "desc": "排除系統元資料檔案: Thumbs.db、desktop.ini、.DS_Store、.Spotlight-V100 等。不會按檔案隱藏屬性或 . 前綴排除使用者自行建立的檔案。"
        },
        "langTemp": {
          "title": "程式語言的暫存檔案",
          "desc": "排除程式語言與建置工具產生的快取和暫存檔案, 例如 JavaScript 的 node_modules、Python 的 __pycache__、Java 的 .gradle 與 *.class。"
        }
      }
    },
    "table": {
      "caption": "項目列表",
      "selectAll": "全選",
      "name": "檔案名稱",
      "size": "大小",
      "strategy": "策略",
      "status": "狀態",
      "empty": "點擊「來源資料夾」選擇目錄, 也可直接拖入資料夾"
    },
    "row": {
      "status": {
        "pendingCompress": "待壓縮",
        "pendingExtract": "待解壓",
        "done": "完成",
        "running": "進行中",
        "failed": "失敗",
        "warning": "警告",
        "meta": "元資料"
      },
      "select": "選擇 {{name}}",
      "original": "原名 {{name}}"
    },
    "dialog": {
      "selectSourceCompress": "選擇要壓縮的資料夾",
      "selectSourceExtract": "選擇要解壓的資料夾",
      "selectDst": "選擇目標資料夾"
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
      "reportIssueFailed": "無法開啟瀏覽器: {{error}}",
      "dialogUnavailable": "資料夾選擇外掛無法使用, 請檢查 tauri-plugin-dialog",
      "dialogFailed": "無法開啟資料夾選擇對話框: {{error}}",
      "dstPicked": "目標資料夾: {{path}}",
      "dropBusy": "任務進行中, 無法更換資料夾",
      "dropWrongZone": "請將資料夾拖到「來源資料夾」或「目標資料夾」",
      "dropOne": "一次只能拖入一個資料夾",
      "dropInvalid": "無法使用拖入的資料夾: {{error}}",
      "dragUnavailable": "資料夾拖曳無法使用: {{error}}",
      "needSelection": "請先勾選要處理的項目",
      "needDst": "請先選擇目標資料夾",
      "needPlan": "打包方案尚未就緒, 請稍候",
      "settingsLoadFailed": "讀取設定失敗, 已使用預設值: {{error}}",
      "settingsLoadWarning": "讀取設定時有問題: {{error}}",
      "settingsSaveFailed": "儲存設定失敗, 已還原: {{error}}",
      "settingsRescan": "排除策略已更新, 正在重新掃描…",
      "modeSwitched": "模式已切換, 請重新選擇來源資料夾",
      "cancelled": "已取消(完成 {{done}}/{{total}})",
      "compressDone_other": "壓縮完成: {{count}} 個壓縮檔 → {{dst}}",
      "extractDone_other": "解壓完成: {{count}} 組 → {{dst}}",
      "compressFailed": "壓縮失敗: {{error}}",
      "extractFailed": "解壓失敗: {{error}}",
      "warnings_other": "; {{count}} 個警告: {{first}}"
    },
    "size": {
      "b": "B",
      "kb": "KB",
      "mb": "MB",
      "gb": "GB"
    }
  }
};
