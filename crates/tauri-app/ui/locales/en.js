window.__ZIPTOOL_LOCALES__ ??= {};
window.__ZIPTOOL_LOCALES__["en"] = {
  "translation": {
    "header": {
      "compress": "Compress",
      "extract": "Extract",
      "confirm": "Confirm",
      "cancel": "Cancel",
      "language": "Language"
    },
    "lang": {
      "system": "Follow system"
    },
    "path": {
      "src": "Source folder",
      "srcPlaceholder": "Click to select, or drop the folder to compress / extract",
      "dst": "Destination folder",
      "dstPlaceholder": "Click to select, or drop the destination folder",
      "dropRelease": "Release to choose this folder"
    },
    "options": {
      "split": "Split",
      "splitPlaceholder": "e.g. 2g / 500m, leave empty for no split",
      "password": "Password",
      "passwordPlaceholder": "Leave empty for no encryption",
      "randomName": "Random archive names",
      "verify": "Verify after compression",
      "verifyTitle": "Check archive structure after compression to catch incomplete archives early",
      "preset": "Mode",
      "presetCompat": "Maximum compatibility",
      "presetCompatTitle": "zip format + maximum compression; opens on any system",
      "presetBalanced": "Balanced",
      "presetBalancedTitle": "7z format (LZMA2), higher compression; requires 7z tools to extract",
      "presetCustom": "Custom",
      "advanced": "Advanced",
      "format": "Format",
      "level": "Level",
      "levelPlaceholder": "Default",
      "levelTitle": "Leave empty for backend default (zip 6 / 7z 5)",
      "exclude": "Exclude",
      "excludePlaceholder": "Comma-separated, e.g. Thumbs.db, *.tmp",
      "force": "Overwrite existing files",
      "pwHintZip": "zip format: ZipCrypto encryption (best compatibility, weaker)",
      "pwHint7z": "7z format: AES-256 encryption"
    },
    "table": {
      "caption": "Item list",
      "selectAll": "Select all",
      "name": "Name",
      "size": "Size",
      "status": "Status",
      "empty": "Click Source folder to select a directory, or drop a folder here"
    },
    "row": {
      "status": {
        "pendingCompress": "Pending compression",
        "pendingExtract": "Pending extraction",
        "done": "Done",
        "running": "In progress",
        "failed": "Failed",
        "warning": "Warning",
        "meta": "Metadata"
      },
      "select": "Select {{name}}",
      "original": "Original name: {{name}}"
    },
    "dialog": {
      "selectSourceCompress": "Select folder to compress",
      "selectSourceExtract": "Select folder to extract",
      "selectDst": "Select destination folder"
    },
    "status": {
      "ready": "Ready",
      "scanning": "Scanning {{done}}/{{total}} {{entry}}",
      "scanningStart": "Scanning…",
      "compressing": "Compressing {{done}}/{{total}} {{entry}} ({{doneBytes}} / {{totalBytes}})",
      "extracting": "Extracting {{done}}/{{total}} {{entry}} ({{doneBytes}} / {{totalBytes}})",
      "scanDone_one": "Scanned {{path}}: {{count}} item, {{bytes}} total",
      "scanDone_other": "Scanned {{path}}: {{count}} items, {{bytes}} total",
      "previewScan_one": "Scanned {{path}}: {{count}} item (browser preview mock data)",
      "previewScan_other": "Scanned {{path}}: {{count}} items (browser preview mock data)",
      "scanFailed": "Scan failed: {{error}}",
      "dialogUnavailable": "Folder picker plugin unavailable; check tauri-plugin-dialog",
      "dialogFailed": "Could not open folder picker: {{error}}",
      "dstPicked": "Destination folder: {{path}}",
      "dropBusy": "A task is running; cannot change folders",
      "dropWrongZone": "Drop the folder onto Source folder or Destination folder",
      "dropOne": "Drop only one folder at a time",
      "dropInvalid": "Cannot use the dropped folder: {{error}}",
      "dragUnavailable": "Folder drag-and-drop unavailable: {{error}}",
      "needSelection": "Select items to process first",
      "needDst": "Select a destination folder first",
      "modeSwitched": "Mode changed; select the source folder again",
      "cancelled": "Cancelled (completed {{done}}/{{total}})",
      "compressDone_one": "Compression complete: {{count}} archive → {{dst}}",
      "compressDone_other": "Compression complete: {{count}} archives → {{dst}}",
      "extractDone_one": "Extraction complete: {{count}} group → {{dst}}",
      "extractDone_other": "Extraction complete: {{count}} groups → {{dst}}",
      "compressFailed": "Compression failed: {{error}}",
      "extractFailed": "Extraction failed: {{error}}",
      "warnings_one": "; {{count}} warning: {{first}}",
      "warnings_other": "; {{count}} warnings: {{first}}"
    },
    "size": {
      "b": "B",
      "kb": "KB",
      "mb": "MB",
      "gb": "GB"
    }
  }
};
