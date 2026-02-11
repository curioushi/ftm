// FTM Web UI - Internationalization
// ===========================================================================

// eslint-disable-next-line no-unused-vars, no-redeclare
var I18N = (function () {
  'use strict';

  var LANG_STORAGE_KEY = 'ftm-lang';
  var currentLang = 'en';

  var dictionaries = {
    en: {
      // -- toolbar --
      'app.title': 'FTM - File Time Machine',
      'toolbar.help': 'Help / Shortcuts',
      'toolbar.helpAriaLabel': 'Help',
      'toolbar.showDeleted': 'Show deleted files',
      'toolbar.scan': 'Scan',
      'toolbar.scanTitle': 'Scan for changes',
      'toolbar.scanning': 'Scanning...',
      // -- sidebar --
      'sidebar.filterPlaceholder': 'filter files...',
      'sidebar.clearFilter': 'Clear filter',
      'sidebar.clearTitle': 'Clear',
      'sidebar.treeDepthTitle': 'Tree depth: drag or click',
      'sidebar.collapseAll': 'Collapse all',
      'sidebar.depth1': 'Depth 1',
      'sidebar.depth2': 'Depth 2',
      'sidebar.depth3': 'Depth 3',
      'sidebar.expandAll': 'Expand all',
      // -- content --
      'content.selectFile': 'Select a file to view history',
      'content.restoreTitle': 'Restore selected version',
      'content.restore': 'Restore',
      // -- timeline range buttons --
      'timeline.rangeAll': 'All',
      'timeline.range1h': '1h',
      'timeline.range6h': '6h',
      'timeline.range1d': '1d',
      'timeline.range3d': '3d',
      'timeline.range7d': '7d',
      'timeline.range30d': '30d',
      // -- timeline --
      'timeline.filterPlaceholder': 'filter files...',
      'timeline.filterLanes': 'Filter timeline lanes',
      // -- empty / loading states --
      'state.noFiles': 'No files',
      'state.noHistory': 'No history',
      'state.noActivity': 'No activity',
      'state.noChanges': 'No changes',
      'state.fileDeleted': 'File was deleted in this version',
      'state.loadingHistory': 'Loading history...',
      'state.computingDiff': 'Computing diff...',
      'state.failedSnapshot': '(failed to load snapshot)',
      // -- status --
      'status.scanResult': 'Scan: +{created} ~{modified} -{deleted}',
      'status.historyFailed': 'Failed to load history: {msg}',
      'status.activityFailed': 'Activity load failed: {msg}',
      'status.restoreRequested': 'Restore requested for {file}',
      'status.noCheckout': 'No directory checked out',
      'status.serverUnreachable': 'Server unreachable',
      'status.noConnect': 'Cannot connect to FTM server',
      'status.checkoutHint': 'Run <code>ftm checkout &lt;dir&gt;</code> first',
      // -- confirm --
      'confirm.restore': 'Restore to version {v}?',
      // -- restore modal --
      'restore.title': 'Restore Preview',
      'restore.current': 'Current',
      'restore.after': 'After Restore',
      'restore.file': 'File',
      'restore.from': 'Current Version',
      'restore.to': 'Restore To',
      'restore.linesAdded': '+{n} lines',
      'restore.linesRemoved': '-{n} lines',
      'restore.cancel': 'Cancel',
      'restore.confirm': 'Confirm Restore',
      'restore.loading': 'Loading diff...',
      'restore.noChanges': 'No changes between these versions',
      'restore.closeHint': 'Press <kbd>Esc</kbd> or click outside to close.',
      // -- diff --
      'diff.unchangedLines': '\u00B7\u00B7\u00B7 {n} unchanged lines \u00B7\u00B7\u00B7',
      'diff.filesSelected': '{n} files selected',
      // -- ops --
      'op.create': 'Create',
      'op.modify': 'Modify',
      'op.delete': 'Delete',
      // -- help modal --
      'help.title': 'Help',
      'help.intro':
        "FTM tracks file changes over time in your project. It lets you browse any file's history, see what changed between versions, and restore a previous version when you need to undo or recover.",
      'help.shortcuts': 'Shortcuts',
      'help.fileList': 'File list',
      'help.multiSelect': 'multi-select files',
      'help.changeFile': 'change selected file',
      'help.timeRange': 'time range (All, 1h, 6h, 1d, 3d, 7d, 30d)',
      'help.fitTimeRange': 'fit time range to current file list (filtered)',
      'help.timelineMouse': 'Timeline (mouse over)',
      'help.panLR': 'pan left / right',
      'help.zoomIO': 'zoom in / out',
      'help.prevNext': 'previous / next version',
      'help.switchLane': 'switch lane (multi-file)',
      'help.timelineWheel': 'Timeline wheel',
      'help.hPan': 'horizontal pan',
      'help.zoomAtMouse': 'zoom at mouse',
      'help.closeHint': 'Press <kbd>Esc</kbd> or click outside to close.',
      // -- lang --
      'lang.en': 'EN',
      'lang.zhCN': '\u4E2D\u6587',
    },
    'zh-CN': {
      // -- toolbar --
      'app.title': 'FTM - \u6587\u4EF6\u65F6\u5149\u673A',
      'toolbar.help': '\u5E2E\u52A9 / \u5FEB\u6377\u952E',
      'toolbar.helpAriaLabel': '\u5E2E\u52A9',
      'toolbar.showDeleted': '\u663E\u793A\u5DF2\u5220\u9664\u6587\u4EF6',
      'toolbar.scan': '\u626B\u63CF',
      'toolbar.scanTitle': '\u626B\u63CF\u53D8\u66F4',
      'toolbar.scanning': '\u626B\u63CF\u4E2D...',
      // -- sidebar --
      'sidebar.filterPlaceholder': '\u7B5B\u9009\u6587\u4EF6...',
      'sidebar.clearFilter': '\u6E05\u9664\u7B5B\u9009',
      'sidebar.clearTitle': '\u6E05\u9664',
      'sidebar.treeDepthTitle': '\u76EE\u5F55\u5C42\u7EA7\uFF1A\u62D6\u52A8\u6216\u70B9\u51FB',
      'sidebar.collapseAll': '\u5168\u90E8\u6298\u53E0',
      'sidebar.depth1': '\u5C42\u7EA7 1',
      'sidebar.depth2': '\u5C42\u7EA7 2',
      'sidebar.depth3': '\u5C42\u7EA7 3',
      'sidebar.expandAll': '\u5168\u90E8\u5C55\u5F00',
      // -- content --
      'content.selectFile': '\u9009\u62E9\u6587\u4EF6\u67E5\u770B\u5386\u53F2',
      'content.restoreTitle': '\u6062\u590D\u9009\u4E2D\u7248\u672C',
      'content.restore': '\u6062\u590D',
      // -- timeline range buttons --
      'timeline.rangeAll': '\u5168\u90E8',
      'timeline.range1h': '1\u5C0F\u65F6',
      'timeline.range6h': '6\u5C0F\u65F6',
      'timeline.range1d': '1\u5929',
      'timeline.range3d': '3\u5929',
      'timeline.range7d': '7\u5929',
      'timeline.range30d': '30\u5929',
      // -- timeline --
      'timeline.filterPlaceholder': '\u7B5B\u9009\u6587\u4EF6...',
      'timeline.filterLanes': '\u7B5B\u9009\u65F6\u95F4\u7EBF\u901A\u9053',
      // -- empty / loading states --
      'state.noFiles': '\u65E0\u6587\u4EF6',
      'state.noHistory': '\u65E0\u5386\u53F2\u8BB0\u5F55',
      'state.noActivity': '\u65E0\u6D3B\u52A8',
      'state.noChanges': '\u65E0\u53D8\u66F4',
      'state.fileDeleted': '\u6587\u4EF6\u5728\u6B64\u7248\u672C\u4E2D\u5DF2\u88AB\u5220\u9664',
      'state.loadingHistory': '\u52A0\u8F7D\u5386\u53F2\u4E2D...',
      'state.computingDiff': '\u8BA1\u7B97\u5DEE\u5F02\u4E2D...',
      'state.failedSnapshot': '(\u52A0\u8F7D\u5FEB\u7167\u5931\u8D25)',
      // -- status --
      'status.scanResult': '\u626B\u63CF: +{created} ~{modified} -{deleted}',
      'status.historyFailed': '\u52A0\u8F7D\u5386\u53F2\u5931\u8D25: {msg}',
      'status.activityFailed': '\u52A0\u8F7D\u6D3B\u52A8\u5931\u8D25: {msg}',
      'status.restoreRequested': '\u5DF2\u8BF7\u6C42\u6062\u590D {file}',
      'status.noCheckout': '\u672A\u68C0\u51FA\u76EE\u5F55',
      'status.serverUnreachable': '\u670D\u52A1\u5668\u4E0D\u53EF\u8FBE',
      'status.noConnect': '\u65E0\u6CD5\u8FDE\u63A5\u5230 FTM \u670D\u52A1\u5668',
      'status.checkoutHint': '\u8BF7\u5148\u8FD0\u884C <code>ftm checkout &lt;dir&gt;</code>',
      // -- confirm --
      'confirm.restore': '\u786E\u8BA4\u6062\u590D\u5230\u7248\u672C {v}\uFF1F',
      // -- restore modal --
      'restore.title': '\u6062\u590D\u9884\u89C8',
      'restore.current': '\u5F53\u524D\u7248\u672C',
      'restore.after': '\u6062\u590D\u540E',
      'restore.file': '\u6587\u4EF6',
      'restore.from': '\u5F53\u524D\u7248\u672C',
      'restore.to': '\u6062\u590D\u76EE\u6807',
      'restore.linesAdded': '+{n} \u884C',
      'restore.linesRemoved': '-{n} \u884C',
      'restore.cancel': '\u53D6\u6D88',
      'restore.confirm': '\u786E\u8BA4\u6062\u590D',
      'restore.loading': '\u52A0\u8F7D\u5DEE\u5F02\u4E2D...',
      'restore.noChanges': '\u4E24\u4E2A\u7248\u672C\u4E4B\u95F4\u6CA1\u6709\u53D8\u66F4',
      'restore.closeHint': '\u6309 <kbd>Esc</kbd> \u6216\u70B9\u51FB\u5916\u90E8\u5173\u95ED\u3002',
      // -- diff --
      'diff.unchangedLines': '\u00B7\u00B7\u00B7 {n} \u884C\u672A\u53D8\u66F4 \u00B7\u00B7\u00B7',
      'diff.filesSelected': '\u5DF2\u9009\u62E9 {n} \u4E2A\u6587\u4EF6',
      // -- ops --
      'op.create': '\u521B\u5EFA',
      'op.modify': '\u4FEE\u6539',
      'op.delete': '\u5220\u9664',
      // -- help modal --
      'help.title': '\u5E2E\u52A9',
      'help.intro':
        'FTM \u8DDF\u8E2A\u9879\u76EE\u4E2D\u6587\u4EF6\u7684\u53D8\u66F4\u5386\u53F2\u3002\u60A8\u53EF\u4EE5\u6D4F\u89C8\u4EFB\u4F55\u6587\u4EF6\u7684\u5386\u53F2\u8BB0\u5F55\uFF0C\u67E5\u770B\u7248\u672C\u4E4B\u95F4\u7684\u5DEE\u5F02\uFF0C\u5E76\u5728\u9700\u8981\u64A4\u9500\u6216\u6062\u590D\u65F6\u8FD8\u539F\u5230\u4E4B\u524D\u7684\u7248\u672C\u3002',
      'help.shortcuts': '\u5FEB\u6377\u952E',
      'help.fileList': '\u6587\u4EF6\u5217\u8868',
      'help.multiSelect': '\u591A\u9009\u6587\u4EF6',
      'help.changeFile': '\u5207\u6362\u9009\u4E2D\u6587\u4EF6',
      'help.timeRange':
        '\u65F6\u95F4\u8303\u56F4 (\u5168\u90E8, 1\u5C0F\u65F6, 6\u5C0F\u65F6, 1\u5929, 3\u5929, 7\u5929, 30\u5929)',
      'help.fitTimeRange':
        '\u6839\u636E\u5F53\u524D\u6587\u4EF6\u5217\u8868\uFF08\u7B5B\u9009\u540E\uFF09\u81EA\u52A8\u8C03\u8282\u65F6\u95F4\u8303\u56F4',
      'help.timelineMouse': '\u65F6\u95F4\u7EBF (\u9F20\u6807\u60AC\u505C)',
      'help.panLR': '\u5DE6\u53F3\u5E73\u79FB',
      'help.zoomIO': '\u653E\u5927 / \u7F29\u5C0F',
      'help.prevNext': '\u4E0A\u4E00\u4E2A / \u4E0B\u4E00\u4E2A\u7248\u672C',
      'help.switchLane': '\u5207\u6362\u901A\u9053 (\u591A\u6587\u4EF6)',
      'help.timelineWheel': '\u65F6\u95F4\u7EBF\u6EDA\u8F6E',
      'help.hPan': '\u6C34\u5E73\u5E73\u79FB',
      'help.zoomAtMouse': '\u4EE5\u9F20\u6807\u4E3A\u4E2D\u5FC3\u7F29\u653E',
      'help.closeHint': '\u6309 <kbd>Esc</kbd> \u6216\u70B9\u51FB\u5916\u90E8\u5173\u95ED\u3002',
      // -- lang --
      'lang.en': 'EN',
      'lang.zhCN': '\u4E2D\u6587',
    },
  };

  /**
   * Translate a key with optional parameter interpolation.
   * @param {string} key - The translation key
   * @param {Object} [params] - Key-value pairs for {var} replacement
   * @returns {string}
   */
  function t(key, params) {
    var dict = dictionaries[currentLang] || dictionaries['en'];
    var str = dict[key];
    if (str === undefined) {
      // Fallback to English
      str = dictionaries['en'][key];
    }
    if (str === undefined) {
      return key; // Return the key itself as last resort
    }
    if (params) {
      Object.keys(params).forEach(function (k) {
        str = str.replace(new RegExp('\\{' + k + '\\}', 'g'), params[k]);
      });
    }
    return str;
  }

  /**
   * Get the current language code.
   * @returns {string}
   */
  function getLang() {
    return currentLang;
  }

  /**
   * Set the active language and apply translations to DOM.
   * @param {string} lang - Language code ('en' or 'zh-CN')
   */
  function setLang(lang) {
    if (!dictionaries[lang]) lang = 'en';
    currentLang = lang;
    localStorage.setItem(LANG_STORAGE_KEY, lang);
    document.documentElement.lang = lang;
    applyI18n();
  }

  /**
   * Walk the DOM and update elements that have data-i18n-* attributes.
   */
  function applyI18n() {
    var propMap = [
      ['data-i18n', 'textContent'],
      ['data-i18n-html', 'innerHTML'],
      ['data-i18n-title', 'title'],
      ['data-i18n-placeholder', 'placeholder'],
    ];
    for (var a = 0; a < propMap.length; a++) {
      var attr = propMap[a][0];
      var prop = propMap[a][1];
      var els = document.querySelectorAll('[' + attr + ']');
      for (var i = 0; i < els.length; i++) {
        els[i][prop] = t(els[i].getAttribute(attr));
      }
    }

    // aria-label requires setAttribute instead of direct property assignment
    var ariaEls = document.querySelectorAll('[data-i18n-aria-label]');
    for (var j = 0; j < ariaEls.length; j++) {
      ariaEls[j].setAttribute('aria-label', t(ariaEls[j].getAttribute('data-i18n-aria-label')));
    }

    document.title = t('app.title');

    var langSelect = document.getElementById('lang-select');
    if (langSelect) {
      langSelect.value = currentLang;
    }
  }

  /**
   * Detect initial language from localStorage or browser preference.
   */
  function detectLang() {
    var saved = localStorage.getItem(LANG_STORAGE_KEY);
    if (saved && dictionaries[saved]) {
      currentLang = saved;
      return;
    }
    var nav = navigator.language || navigator.userLanguage || '';
    if (nav.toLowerCase().indexOf('zh') === 0) {
      currentLang = 'zh-CN';
    } else {
      currentLang = 'en';
    }
  }

  // Auto-detect on load
  detectLang();

  return {
    t: t,
    getLang: getLang,
    setLang: setLang,
    applyI18n: applyI18n,
  };
})();
