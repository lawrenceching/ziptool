// TEMP: 由 scripts/scaffold-locales.mjs 生成的 zh-CN 占位词条, Task 7 必须替换为用户提供的最终文案。
window.__ZIPTOOL_LOCALES__ ??= {};
window.__ZIPTOOL_LOCALES__["zh-TW"] = {
  "translation": {
    "header": {
      "compress": "压缩",
      "extract": "解压",
      "confirm": "确认",
      "cancel": "取消",
      "language": "语言"
    },
    "lang": {
      "system": "跟随系统"
    },
    "path": {
      "src": "源文件夹",
      "srcPlaceholder": "点击选择, 或拖入要压缩 / 解压的文件夹",
      "dst": "目标文件夹",
      "dstPlaceholder": "点击选择, 或拖入目标文件夹",
      "dropRelease": "松开鼠标以选择此文件夹"
    },
    "options": {
      "split": "分包",
      "splitPlaceholder": "如 2g / 500m,留空不分包",
      "password": "密码",
      "passwordPlaceholder": "留空不加密",
      "randomName": "随机包名",
      "verify": "压缩成功后校验",
      "verifyTitle": "压缩完成后检查压缩包结构, 提前发现不完整的包",
      "preset": "模式",
      "presetCompat": "最大兼容性",
      "presetCompatTitle": "zip 格式 + 最大压缩强度; 所有系统都能打开",
      "presetBalanced": "平衡",
      "presetBalancedTitle": "7z 格式(LZMA2), 压缩率更高; 需 7z 系工具解压",
      "presetCustom": "自定义",
      "advanced": "高级",
      "format": "格式",
      "level": "强度",
      "levelPlaceholder": "默认",
      "levelTitle": "留空 = 后端默认强度(zip 6 / 7z 5)",
      "exclude": "排除",
      "excludePlaceholder": "逗号分隔, 如 Thumbs.db, *.tmp",
      "force": "覆盖已有文件",
      "pwHintZip": "zip 格式: ZipCrypto 加密(兼容性最好, 强度弱)",
      "pwHint7z": "7z 格式: AES-256 加密"
    },
    "table": {
      "caption": "条目列表",
      "selectAll": "全选",
      "name": "文件名",
      "size": "大小",
      "status": "状态",
      "empty": "点击「源文件夹」选择目录, 也可直接拖入文件夹"
    },
    "row": {
      "status": {
        "pendingCompress": "待压缩",
        "pendingExtract": "待解压",
        "done": "完成",
        "running": "进行中",
        "failed": "失败",
        "warning": "告警",
        "meta": "元数据"
      },
      "select": "选择 {{name}}",
      "original": "原名 {{name}}"
    },
    "dialog": {
      "selectSourceCompress": "选择要压缩的文件夹",
      "selectSourceExtract": "选择要解压的文件夹",
      "selectDst": "选择目标文件夹"
    },
    "status": {
      "ready": "就绪",
      "scanning": "扫描中 {{done}}/{{total}} {{entry}}",
      "scanningStart": "扫描中…",
      "compressing": "压缩中 {{done}}/{{total}} {{entry}} ({{doneBytes}} / {{totalBytes}})",
      "extracting": "解压中 {{done}}/{{total}} {{entry}} ({{doneBytes}} / {{totalBytes}})",
      "scanDone_other": "已扫描 {{path}}: {{count}} 个条目, 共 {{bytes}}",
      "previewScan_other": "已扫描 {{path}}: {{count}} 个条目(浏览器预览假数据)",
      "scanFailed": "扫描失败: {{error}}",
      "dialogUnavailable": "文件夹选择插件不可用, 请检查 tauri-plugin-dialog",
      "dialogFailed": "无法打开文件夹选择框: {{error}}",
      "dstPicked": "目标文件夹: {{path}}",
      "dropBusy": "任务进行中, 无法更换文件夹",
      "dropWrongZone": "请将文件夹拖到「源文件夹」或「目标文件夹」",
      "dropOne": "一次只能拖入一个文件夹",
      "dropInvalid": "无法使用拖入的文件夹: {{error}}",
      "dragUnavailable": "文件夹拖拽不可用: {{error}}",
      "needSelection": "请先勾选要处理的条目",
      "needDst": "请先选择目标文件夹",
      "modeSwitched": "模式已切换, 请重新选择源文件夹",
      "cancelled": "已取消(完成 {{done}}/{{total}})",
      "compressDone_other": "压缩完成: {{count}} 个压缩包 → {{dst}}",
      "extractDone_other": "解压完成: {{count}} 组 → {{dst}}",
      "compressFailed": "压缩失败: {{error}}",
      "extractFailed": "解压失败: {{error}}",
      "warnings_other": "; {{count}} 条告警: {{first}}"
    },
    "size": {
      "b": "B",
      "kb": "KB",
      "mb": "MB",
      "gb": "GB"
    }
  }
};
