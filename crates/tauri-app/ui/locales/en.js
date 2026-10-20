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
      "pwHintZip": "zip format: ZipCrypto encryption (best compatibility, less secure)",
      "pwHint7z": "7z format: AES-256 encryption"
    },
    "strategy": {
      "label": "Packaging",
      "mergeBelow": "Merge ≤",
      "mergeBelowPlaceholder": "100m",
      "mergeBelowTitle": "Top-level items at or below this size enter the merge pool; leave empty to disable merging",
      "splitAbove": "Split >",
      "splitAbovePlaceholder": "2g",
      "splitAboveTitle": "Items larger than this are split into volumes of this size; leave empty to disable splitting",
      "bundleMax": "Bundle ≤",
      "bundleMaxPlaceholder": "Follow split",
      "bundleMaxTitle": "Maximum raw size of one merged bundle; leave empty to follow the split threshold",
      "maxEntries": "Entries ≤",
      "maxEntriesPlaceholder": "50000",
      "maxEntriesTitle": "Maximum archive entries in one merged bundle; leave empty for no limit",
      "viewPlan": "View plan"
    },
    "plan": {
      "title": "Packaging plan",
      "close": "Close",
      "strategyLine": "Strategy: merge ≤ {{merge}} · split > {{split}} · bundle ≤ {{bundle}} · entries ≤ {{entries}}",
      "filter": { "all": "All", "direct": "Direct", "split": "Split", "bundle": "Merged" },
      "kind": {
        "direct": "Direct",
        "split": "Split",
        "bundle": "Merged",
        "bundleN": "Merged #{{n}}",
        "directOver": "Direct · over limit"
      },
      "reason": {
        "split": "{{size}} > split threshold {{limit}}",
        "merge": "{{size}} ≤ merge threshold {{limit}}",
        "direct": "{{size}} did not enter the merge pool"
      },
      "summary": {
        "idle": "Select a source folder to generate a packaging plan",
        "noSelection": "No items selected",
        "planning": "Calculating packaging plan…",
        "error": "Plan invalid: {{error}}",
        "text": "Expected {{total}} logical packages ({{direct}} direct · {{split}} split · {{bundle}} merged) · max {{max}}",
        "splitNote": "{{count}} split package(s) at {{limit}}",
        "warnings": "{{count}} notice(s)"
      },
      "packageMeta": "{{size}} · {{members}} item(s) · {{entries}} entries",
      "moreMembers": "{{count}} more member(s)",
      "morePackages": "{{count}} more package(s) not shown; narrow the filter",
      "empty": "No packages match this filter",
      "splitCardNote": "Single item above the split threshold; compressed into volumes of {{limit}}, actual count depends on compressed size",
      "note": "Sizes are raw; compressed size and actual volume count are unknown.",
      "warning": {
        "tooManyEntries": "{{name}} has {{count}} entries, over the limit {{limit}}; changed to direct compression"
      }
    },
    "settings": {
      "title": "Settings",
      "open": "Settings",
      "back": "Back",
      "nav": {
        "label": "Settings navigation",
        "general": "General",
        "filters": "File exclusions"
      },
      "general": {
        "heading": "General",
        "shutdown": {
          "title": "Shut down after compression",
          "desc": "Coming soon: automatic shutdown after a successful compression will be available in a future version."
        }
      },
      "filters": {
        "heading": "File exclusions",
        "intro": "Policies apply when scanning for compression; changing them rescans the current source folder when you close Settings. Excluded items remain visible but are unchecked by default. Per-run custom patterns are still available under Advanced on the main screen.",
        "osHidden": {
          "title": "OS hidden files",
          "desc": "Exclude system metadata files such as Thumbs.db, desktop.ini, .DS_Store, and .Spotlight-V100. User files are not excluded based on hidden attributes or dot prefixes."
        },
        "langTemp": {
          "title": "Language temporary files",
          "desc": "Exclude caches and temporary files generated by languages and build tools, such as JavaScript node_modules, Python __pycache__, and Java .gradle and *.class."
        }
      }
    },
    "table": {
      "caption": "Item list",
      "selectAll": "Select all",
      "name": "Name",
      "size": "Size",
      "package": "Package",
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
      "needPlan": "Packaging plan is not ready yet; please wait",
      "settingsLoadFailed": "Could not load settings; using defaults: {{error}}",
      "settingsLoadWarning": "Issue while loading settings: {{error}}",
      "settingsSaveFailed": "Could not save settings; reverted: {{error}}",
      "settingsRescan": "Exclusion policies changed; rescanning…",
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
