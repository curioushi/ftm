// FTM Web UI - Application Logic
// ===========================================================================

(function () {
  'use strict';

  // ---- State ---------------------------------------------------------------
  let fileTree = []; // raw tree from API [{name, count?, children?}]
  let currentFile = null; // selected file path
  let historyEntries = []; // history for current file
  let activeEntryIdx = -1; // selected timeline node index
  let diffRows = []; // flat array of rendered diff row data
  let collapsedDirs = new Set(); // collapsed directory paths
  let hideDeletedFiles = true; // when true, API returns only files not deleted
  let lastDiffFromChecksum = null;
  let lastDiffToChecksum = null;
  let visibleFilePaths = [];
  let diffSingleMode = false;
  let selectedRestoreChecksum = null;
  let selectedFiles = new Set(); // multi-selected files (ctrl+click)
  let tlTooltipTimeoutId = null; // timeout for auto-hiding keyboard tooltip
  let isMouseOverTimeline = false; // track if mouse is over the timeline area
  const SELECTED_FILE_STORAGE_KEY = 'ftm-selected-file';
  const SELECTED_FILES_STORAGE_KEY = 'ftm-selected-files';
  const TIMELINE_RANGE_STORAGE_KEY = 'ftm-timeline-range';
  const TL_HEIGHT_STORAGE_KEY = 'ftm-timeline-height';
  const TL_LANES_WIDTH_STORAGE_KEY = 'ftm-tl-lanes-width';
  const TREE_DEPTH_STORAGE_KEY = 'ftm-tree-depth';
  const SHOW_DELETED_STORAGE_KEY = 'ftm-show-deleted';

  // Timeline state
  let tlViewStart = 0; // ms timestamp (left edge of visible range)
  let tlViewEnd = 0; // ms timestamp (right edge of visible range)
  let tlLanes = []; // [{file, entries}] - lanes to display
  let tlMode = 'single'; // 'single' or 'multi'
  let tlScrollY = 0; // vertical scroll offset for lanes
  let tlDragState = null; // {startX, startY, origViewStart, origViewEnd, origScrollY, moved}
  let tlHoveredNode = null; // {laneIdx, entryIdx, entry, x, y}
  let tlActiveNode = null; // {laneIdx, entryIdx, entry} - currently selected node
  let tlRafId = null;
  let tlActiveRangeBtn = null; // currently active range button element
  let tlLaneFilterQuery = ''; // filter for timeline lanes (file path/name)

  // Canvas drawing constants
  const LANE_HEIGHT = 24;
  const RULER_HEIGHT = 22;
  const NODE_RADIUS = 5;
  const NODE_HIT_RADIUS = 8;
  const MIN_VIEW_SPAN = 60 * 1000; // 1 minute minimum zoom
  const FOCUS_MARGIN = 48; // min padding when panning to keep active node in view

  // Colors (must match CSS variables)
  const COLORS = {
    bg: '#000',
    bgSurface: '#0a0a0a',
    bgHover: '#1a1a1a',
    bgActive: '#222',
    fg: '#e0e0e0',
    fgDim: '#888',
    fgBright: '#fff',
    border: '#333',
    green: '#22c55e',
    blue: '#3b82f6',
    red: '#ef4444',
  };

  function findEntryIndex(entries, timestamp, checksum) {
    for (let i = 0; i < entries.length; i++) {
      if (entries[i].timestamp === timestamp && entries[i].checksum === checksum) return i;
    }
    return -1;
  }

  function shouldShowOnTimeline() {
    return (
      !hideDeletedFiles ||
      historyEntries.length === 0 ||
      historyEntries[historyEntries.length - 1].op !== 'delete'
    );
  }

  function opColor(op) {
    if (op === 'create') return COLORS.green;
    if (op === 'modify') return COLORS.blue;
    if (op === 'delete') return COLORS.red;
    return COLORS.fgDim;
  }

  var t = I18N.t;

  function opLabel(op) {
    if (!op) return '';
    var key = 'op.' + op.toLowerCase();
    return t(key);
  }

  // ---- DOM refs ------------------------------------------------------------
  const $filter = document.getElementById('filter');
  const $showDeleted = document.getElementById('show-deleted');
  const $fileList = document.getElementById('file-list');
  const $diffViewer = document.getElementById('diff-viewer');
  const $diffTitle = document.getElementById('diff-title');
  const $diffMeta = document.getElementById('diff-meta');
  const $btnRestore = document.getElementById('btn-restore');
  const $timelineLabel = document.getElementById('timeline-label');
  const $btnScan = document.getElementById('btn-scan');
  const $status = document.getElementById('status');
  const $toolbarStats = document.getElementById('toolbar-stats');
  const $statsHistoryFill = document.getElementById('stats-history-fill');
  const $statsHistoryText = document.getElementById('stats-history-text');
  const $statsQuotaFill = document.getElementById('stats-quota-fill');
  const $statsQuotaText = document.getElementById('stats-quota-text');
  const $sidebar = document.getElementById('sidebar');
  const $resizeHandle = document.getElementById('resize-handle');

  // New timeline DOM refs
  const $timelineBar = document.getElementById('timeline-bar');
  const $timelineBody = document.getElementById('timeline-body');
  const $timelineLanes = document.getElementById('timeline-lanes');
  const $canvas = document.getElementById('timeline-canvas');
  const $tlTooltip = document.getElementById('timeline-tooltip');
  const $tlResizeHandle = document.getElementById('timeline-resize-handle');
  const $tlLanesResize = document.getElementById('timeline-lanes-resize');
  const ctx = $canvas.getContext('2d');
  const $btnHelp = document.getElementById('btn-help');
  const $helpOverlay = document.getElementById('help-overlay');
  const $helpTitle = document.getElementById('help-title');

  // Restore modal DOM refs
  const $restoreOverlay = document.getElementById('restore-overlay');
  const $restoreFilepath = document.getElementById('restore-filepath');
  const $restoreInfoFilename = document.getElementById('restore-info-filename');
  const $restoreInfoFrom = document.getElementById('restore-info-from');
  const $restoreInfoTo = document.getElementById('restore-info-to');
  const $restoreInfoStats = document.getElementById('restore-info-stats');
  const $restorePanelLeft = document.getElementById('restore-panel-left');
  const $restorePanelRight = document.getElementById('restore-panel-right');
  const $restoreCancel = document.getElementById('restore-cancel');
  const $restoreConfirm = document.getElementById('restore-confirm');

  // ---- API helpers ---------------------------------------------------------
  const API = '';

  async function api(path) {
    const res = await fetch(API + path);
    if (!res.ok) {
      const body = await res.json().catch(() => ({ message: res.statusText }));
      throw new Error(body.message || res.statusText);
    }
    return res;
  }

  async function apiJson(path) {
    const res = await api(path);
    return res.json();
  }

  async function apiPost(path, body) {
    const res = await fetch(API + path, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: body ? JSON.stringify(body) : undefined,
    });
    if (!res.ok) {
      const data = await res.json().catch(() => ({ message: res.statusText }));
      throw new Error(data.message || res.statusText);
    }
    return res.json();
  }

  function formatNumber(n) {
    if (n >= 1_000_000) return (n / 1_000_000).toFixed(1).replace(/\.0$/, '') + 'M';
    if (n >= 1_000) return (n / 1_000).toFixed(1).replace(/\.0$/, '') + 'k';
    return String(n);
  }

  function formatBytes(n) {
    const KB = 1024;
    const MB = KB * 1024;
    const GB = MB * 1024;
    if (n >= GB) return (n / GB).toFixed(1).replace(/\.0$/, '') + ' GB';
    if (n >= MB) return (n / MB).toFixed(1).replace(/\.0$/, '') + ' MB';
    if (n >= KB) return (n / KB).toFixed(1).replace(/\.0$/, '') + ' KB';
    return n + ' B';
  }

  async function loadStats() {
    if (
      !$toolbarStats ||
      !$statsHistoryFill ||
      !$statsHistoryText ||
      !$statsQuotaFill ||
      !$statsQuotaText
    )
      return;
    try {
      const st = await apiJson('/api/stats');
      const historyPct =
        st.max_history > 0 ? Math.min(100, (st.history / st.max_history) * 100) : 0;
      const quotaPct = st.max_quota > 0 ? Math.min(100, (st.quota / st.max_quota) * 100) : 0;
      $statsHistoryFill.style.width = historyPct + '%';
      $statsHistoryText.textContent =
        formatNumber(st.history) + ' / ' + formatNumber(st.max_history);
      $statsQuotaFill.style.width = quotaPct + '%';
      $statsQuotaText.textContent = formatBytes(st.quota) + ' / ' + formatBytes(st.max_quota);
    } catch {
      $statsHistoryFill.style.width = '0%';
      $statsHistoryText.textContent = '0 / 0';
      $statsQuotaFill.style.width = '0%';
      $statsQuotaText.textContent = '0 B / 0 B';
    }
  }

  // ---- File list -----------------------------------------------------------

  async function loadFiles() {
    try {
      const includeDeleted = !hideDeletedFiles;
      const q = includeDeleted ? '?include_deleted=true' : '';
      fileTree = await apiJson('/api/files' + q);
      applyDepthByIndex(currentDepthIndex);
      renderFileList();
    } catch (e) {
      $status.textContent = e.message;
    }
  }

  function createFilterMatcher(rawQuery) {
    const query = rawQuery.trim().toLowerCase().replace(/\\/g, '/');
    if (!query) {
      return null;
    }
    const hasGlob = /[*?]/.test(query);
    if (!hasGlob) {
      return (path) => path.toLowerCase().includes(query);
    }
    const regex = globToRegex(query);
    return (path) => regex.test(path.toLowerCase().replace(/\\/g, '/'));
  }

  function globToRegex(pattern) {
    let out = '^';
    for (let i = 0; i < pattern.length; i++) {
      const ch = pattern[i];
      if (ch === '*') {
        if (pattern[i + 1] === '*') {
          out += '.*';
          i++;
        } else {
          out += '[^/]*';
        }
        continue;
      }
      if (ch === '?') {
        out += '[^/]';
        continue;
      }
      if ('\\.^$+()[]{}|'.includes(ch)) {
        out += '\\' + ch;
        continue;
      }
      out += ch;
    }
    out += '$';
    return new RegExp(out);
  }

  // Filter tree: return new tree with only matching files and their ancestors
  function filterTree(nodes, matcher, prefix) {
    const result = [];
    for (const node of nodes) {
      const fullPath = prefix ? prefix + '/' + node.name : node.name;
      if (node.children) {
        const filteredChildren = filterTree(node.children, matcher, fullPath);
        if (filteredChildren.length > 0) {
          result.push({ name: node.name, children: filteredChildren });
        }
      } else {
        if (matcher(fullPath)) {
          result.push({ name: node.name, count: node.count });
        }
      }
    }
    return result;
  }

  function renderFileList() {
    const matcher = createFilterMatcher($filter.value);
    const tree = matcher ? filterTree(fileTree, matcher, '') : fileTree;

    $fileList.innerHTML = '';
    visibleFilePaths = [];

    if (tree.length === 0) {
      $fileList.innerHTML = '<div class="empty-state">' + escapeHtml(t('state.noFiles')) + '</div>';
      return;
    }

    const frag = document.createDocumentFragment();
    renderTreeNodes(frag, tree, '', 0, !!matcher);
    $fileList.appendChild(frag);
    scrollFileToActive();
  }

  /** Collect all file paths under a tree node recursively */
  function collectFilesUnder(nodes, prefix) {
    const result = [];
    for (const node of nodes) {
      const fullPath = prefix ? prefix + '/' + node.name : node.name;
      if (node.children) {
        result.push(...collectFilesUnder(node.children, fullPath));
      } else {
        result.push(fullPath);
      }
    }
    return result;
  }

  // Recursively render tree nodes into a parent element
  function renderTreeNodes(parent, nodes, prefix, depth, forceExpand) {
    const sorted = nodes.slice().sort((a, b) => {
      const aIsDir = !!a.children;
      const bIsDir = !!b.children;
      if (aIsDir !== bIsDir) return aIsDir ? -1 : 1;
      return (a.name || '').localeCompare(b.name || '', undefined, { sensitivity: 'base' });
    });
    for (const node of sorted) {
      const fullPath = prefix ? prefix + '/' + node.name : node.name;

      if (node.children) {
        // Directory node
        const isCollapsed = !forceExpand && collapsedDirs.has(fullPath);

        const dirRow = document.createElement('div');
        dirRow.className = 'tree-dir-row' + (isCollapsed ? ' collapsed' : '');
        dirRow.style.paddingLeft = 8 + depth * 16 + 'px';

        const arrow = document.createElement('span');
        arrow.className = 'arrow';
        arrow.textContent = '\u25BE';

        const nameSpan = document.createElement('span');
        nameSpan.className = 'dir-name';
        nameSpan.textContent = node.name;

        // Arrow click: toggle expand/collapse
        arrow.addEventListener('click', (e) => {
          e.stopPropagation();
          clearTimelineLaneFilter();
          if (collapsedDirs.has(fullPath)) {
            collapsedDirs.delete(fullPath);
          } else {
            collapsedDirs.add(fullPath);
          }
          renderFileList();
        });

        // Row click (name or blank area): select all files under this directory
        function onDirRowClick() {
          clearTimelineLaneFilter();
          collapsedDirs.delete(fullPath);
          const childFiles = collectFilesUnder(node.children, fullPath);
          if (childFiles.length === 0) return;
          selectedFiles = new Set(childFiles);
          currentFile = childFiles[0];
          rememberSelectedFiles(childFiles);
          renderFileList();
          selectMultipleFiles(childFiles);
        }
        dirRow.addEventListener('click', onDirRowClick);

        dirRow.appendChild(arrow);
        dirRow.appendChild(nameSpan);

        parent.appendChild(dirRow);

        const childContainer = document.createElement('div');
        childContainer.className = 'tree-children' + (isCollapsed ? ' collapsed' : '');
        renderTreeNodes(childContainer, node.children, fullPath, depth + 1, forceExpand);
        parent.appendChild(childContainer);
      } else {
        // File node
        const isActive = fullPath === currentFile;
        const isSelected = selectedFiles.has(fullPath);
        const fileRow = document.createElement('div');
        fileRow.className =
          'tree-file' + (isActive ? ' active' : '') + (isSelected && !isActive ? ' selected' : '');
        fileRow.style.paddingLeft = 8 + depth * 16 + 18 + 'px';
        visibleFilePaths.push(fullPath);

        const nameSpan = document.createElement('span');
        nameSpan.className = 'file-name';
        nameSpan.textContent = node.name;

        const countSpan = document.createElement('span');
        countSpan.className = 'count';
        countSpan.textContent = node.count || 0;

        fileRow.appendChild(nameSpan);
        fileRow.appendChild(countSpan);
        fileRow.addEventListener('click', (e) => {
          clearTimelineLaneFilter();
          if (e.ctrlKey || e.metaKey) {
            // Ctrl/Cmd+click: toggle file in multi-selection
            if (selectedFiles.has(fullPath)) {
              selectedFiles.delete(fullPath);
              // If removing the current file, switch currentFile
              if (currentFile === fullPath) {
                const remaining = Array.from(selectedFiles);
                currentFile = remaining.length > 0 ? remaining[0] : null;
              }
            } else {
              selectedFiles.add(fullPath);
              // Also add the current single file if this is the first ctrl-click
              if (currentFile && selectedFiles.size === 1) {
                selectedFiles.add(currentFile);
              }
            }
            if (selectedFiles.size > 1) {
              renderFileList();
              selectMultipleFiles(Array.from(selectedFiles));
            } else if (selectedFiles.size === 1) {
              const f = Array.from(selectedFiles)[0];
              selectedFiles.clear();
              selectFile(f);
            } else {
              selectedFiles.clear();
              renderFileList();
            }
          } else {
            // Normal click: single file select
            selectedFiles.clear();
            selectFile(fullPath);
          }
        });

        parent.appendChild(fileRow);
      }
    }
  }

  // ---- Tree depth track (o-o-o-x-o discrete draggable) ----------------------
  let currentDepthIndex = 4; // 0=collapse, 1..3=depth, 4=expand

  function collectAllDirPaths(nodes, prefix, depth, result) {
    for (const node of nodes) {
      const fullPath = prefix ? prefix + '/' + node.name : node.name;
      if (node.children) {
        result.push({ path: fullPath, depth: depth });
        collectAllDirPaths(node.children, fullPath, depth + 1, result);
      }
    }
    return result;
  }

  function applyDepthByIndex(index) {
    const dirs = collectAllDirPaths(fileTree, '', 0, []);
    if (index === 0) {
      collapsedDirs.clear();
      for (const d of dirs) {
        collapsedDirs.add(d.path);
      }
    } else if (index === 4) {
      collapsedDirs.clear();
    } else {
      const maxDepth = index;
      collapsedDirs.clear();
      for (const d of dirs) {
        if (d.depth >= maxDepth) {
          collapsedDirs.add(d.path);
        }
      }
    }
    renderFileList();
  }

  function positionTreeDepthThumb() {
    const track = document.getElementById('tree-depth-track');
    const thumb = document.getElementById('tree-depth-thumb');
    if (!track || !thumb) return;
    const rail = track.querySelector('.tree-depth-rail');
    if (!rail) return;
    const trackRect = track.getBoundingClientRect();
    const railRect = rail.getBoundingClientRect();
    const railWidth = railRect.width;
    if (railWidth <= 0) return;
    const railLeft = railRect.left - trackRect.left;
    const centerX = railLeft + (currentDepthIndex / 4) * railWidth;
    thumb.style.left = centerX + 'px';
    track.setAttribute('aria-valuenow', currentDepthIndex);
  }

  function indexFromClientX(clientX) {
    const track = document.getElementById('tree-depth-track');
    const rail = track.querySelector('.tree-depth-rail');
    if (!track || !rail) return currentDepthIndex;
    const railRect = rail.getBoundingClientRect();
    const x = clientX - railRect.left;
    const w = railRect.width;
    if (w <= 0) return currentDepthIndex;
    const ratio = Math.max(0, Math.min(1, x / w));
    return Math.round(ratio * 4);
  }

  function initTreeDepthButtons() {
    const saved = localStorage.getItem(TREE_DEPTH_STORAGE_KEY);
    if (saved !== null) {
      const i = parseInt(saved, 10);
      if (i >= 0 && i <= 4) currentDepthIndex = i;
    }

    const track = document.getElementById('tree-depth-track');
    const thumb = document.getElementById('tree-depth-thumb');
    if (!track || !thumb) return;

    positionTreeDepthThumb();
    applyDepthByIndex(currentDepthIndex);

    track.addEventListener('click', (e) => {
      const dot = e.target.closest('.tree-depth-dot');
      if (!dot) return;
      const i = parseInt(dot.dataset.index, 10);
      if (i < 0 || i > 4) return;
      currentDepthIndex = i;
      positionTreeDepthThumb();
      applyDepthByIndex(currentDepthIndex);
      localStorage.setItem(TREE_DEPTH_STORAGE_KEY, String(currentDepthIndex));
    });

    track.addEventListener('mousedown', (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      track.classList.add('dragging');
      const onMove = (e2) => {
        const i = indexFromClientX(e2.clientX);
        if (i !== currentDepthIndex) {
          currentDepthIndex = i;
          positionTreeDepthThumb();
          applyDepthByIndex(currentDepthIndex);
          localStorage.setItem(TREE_DEPTH_STORAGE_KEY, String(currentDepthIndex));
        }
      };
      const onUp = () => {
        track.classList.remove('dragging');
        document.removeEventListener('mousemove', onMove);
        document.removeEventListener('mouseup', onUp);
      };
      onMove(e);
      document.addEventListener('mousemove', onMove);
      document.addEventListener('mouseup', onUp);
    });

    window.addEventListener('resize', positionTreeDepthThumb);
  }

  // ---- Drag resize helper ---------------------------------------------------
  function initDragResize(opts) {
    const saved = localStorage.getItem(opts.storageKey);
    if (saved) {
      const v = parseInt(saved, 10);
      if (v >= opts.minSize) {
        opts.target.style[opts.sizeProp] = v + 'px';
      }
    }

    let startPos = 0;
    let startSize = 0;

    function onMouseDown(e) {
      e.preventDefault();
      startPos = opts.axis === 'x' ? e.clientX : e.clientY;
      startSize = opts.target.getBoundingClientRect()[opts.sizeProp];
      opts.handle.classList.add('dragging');
      document.body.classList.add(opts.bodyClass);
      document.addEventListener('mousemove', onMouseMove);
      document.addEventListener('mouseup', onMouseUp);
    }

    function onMouseMove(e) {
      const currentPos = opts.axis === 'x' ? e.clientX : e.clientY;
      let delta = currentPos - startPos;
      if (opts.invert) delta = -delta;
      const maxSize = opts.getMaxSize();
      const newSize = Math.max(opts.minSize, Math.min(maxSize, startSize + delta));
      opts.target.style[opts.sizeProp] = newSize + 'px';
      if (opts.onResize) opts.onResize();
    }

    function onMouseUp() {
      opts.handle.classList.remove('dragging');
      document.body.classList.remove(opts.bodyClass);
      document.removeEventListener('mousemove', onMouseMove);
      document.removeEventListener('mouseup', onMouseUp);
      const finalSize = Math.round(opts.target.getBoundingClientRect()[opts.sizeProp]);
      localStorage.setItem(opts.storageKey, String(finalSize));
      if (opts.onResize) opts.onResize();
    }

    opts.handle.addEventListener('mousedown', onMouseDown);
  }

  async function selectFile(path) {
    currentFile = path;
    selectedFiles.clear();
    rememberSelectedFile(path);
    selectedRestoreChecksum = null;
    updateRestoreButton();
    renderFileList();
    $diffTitle.textContent = path;
    $diffMeta.textContent = '';
    $diffViewer.innerHTML =
      '<div class="loading">' + escapeHtml(t('state.loadingHistory')) + '</div>';

    // Clear active range button when selecting a specific file
    clearActiveRangeBtn();

    try {
      historyEntries = await apiJson('/api/history?file=' + encodeURIComponent(path));
      tlMode = 'single';
      setTimelineSingleFile(path, shouldShowOnTimeline() ? historyEntries : []);
      // Auto-select latest entry
      if (historyEntries.length > 0) {
        selectEntry(historyEntries.length - 1);
      } else {
        $diffViewer.innerHTML =
          '<div class="empty-state">' + escapeHtml(t('state.noHistory')) + '</div>';
        updateRestoreButton();
      }
    } catch (e) {
      $diffViewer.innerHTML = '<div class="empty-state">' + escapeHtml(e.message) + '</div>';
      updateRestoreButton();
    }
  }

  /** Select multiple files and show them in the multi-file timeline */
  async function selectMultipleFiles(files) {
    if (files.length === 0) return;
    currentFile = files[0];
    selectedFiles = new Set(files);
    clearActiveRangeBtn();
    $diffTitle.textContent = t('diff.filesSelected', { n: files.length });
    $diffMeta.textContent = '';

    // Load history for all selected files
    try {
      const allEntries = [];
      const results = await Promise.all(
        files.map((f) => apiJson('/api/history?file=' + encodeURIComponent(f)))
      );
      for (let i = 0; i < files.length; i++) {
        const fileEntries = results[i];
        const lastIsDelete =
          fileEntries.length > 0 && fileEntries[fileEntries.length - 1].op === 'delete';
        if (hideDeletedFiles && lastIsDelete) continue;
        for (const entry of fileEntries) {
          allEntries.push(entry);
        }
      }
      if (allEntries.length === 0) {
        tlMode = 'multi';
        tlLanes = [];
        tlViewStart = Date.now() - 3600000;
        tlViewEnd = Date.now();
        updateTimelineLabel();
        updateLaneLabels();
        rememberSelectedFiles(files);
        requestTimelineDraw();
        return;
      }

      // Derive time range from entries
      let minTs = Infinity;
      let maxTs = -Infinity;
      for (const e of allEntries) {
        const ts = new Date(e.timestamp).getTime();
        if (ts < minTs) minTs = ts;
        if (ts > maxTs) maxTs = ts;
      }
      const span = Math.max(maxTs - minTs, MIN_VIEW_SPAN);
      const pad = span * 0.08;
      setTimelineMultiFile(allEntries, minTs - pad, maxTs + pad);
      rememberSelectedFiles(files);

      // Also load history for the primary (currentFile) to enable diff
      if (currentFile) {
        const idx = files.indexOf(currentFile);
        if (idx >= 0) {
          historyEntries = results[idx];
          if (historyEntries.length > 0) {
            selectEntryDiff(historyEntries.length - 1);
          }
        }
      }
    } catch (e) {
      $status.textContent = t('status.historyFailed', { msg: e.message });
    }
  }

  // ---- Timeline Engine (Canvas) ---------------------------------------------
  function updateRestoreButton() {
    if (!currentFile || !selectedRestoreChecksum) {
      $btnRestore.classList.remove('is-visible');
      return;
    }
    $btnRestore.classList.add('is-visible');
  }

  /** Set timeline to show a single file's history */
  function setTimelineSingleFile(file, entries) {
    tlLanes = entries.length > 0 ? [{ file: file, entries: entries }] : [];
    tlScrollY = 0;
    tlActiveNode = null;

    if (entries.length === 0) {
      tlViewStart = Date.now() - 3600000;
      tlViewEnd = Date.now();
      updateTimelineLabel();
      updateLaneLabels();
      requestTimelineDraw();
      return;
    }

    // Set view range to encompass all entries with padding
    const first = new Date(entries[0].timestamp).getTime();
    const last = new Date(entries[entries.length - 1].timestamp).getTime();
    const span = Math.max(last - first, MIN_VIEW_SPAN);
    const pad = span * 0.08;
    tlViewStart = first - pad;
    tlViewEnd = last + pad;

    updateTimelineLabel();
    updateLaneLabels();
    requestTimelineDraw();
  }

  /** Set timeline to show multi-file activity */
  function setTimelineMultiFile(activityEntries, viewStart, viewEnd) {
    // Group entries by file
    const byFile = new Map();
    for (const e of activityEntries) {
      if (!byFile.has(e.file)) {
        byFile.set(e.file, []);
      }
      byFile.get(e.file).push(e);
    }

    // Sort files by latest modification time (newest first); tie-break by path
    const sortedFiles = Array.from(byFile.keys()).sort((a, b) => {
      const entriesA = byFile.get(a);
      const entriesB = byFile.get(b);
      const maxTsA = Math.max(...entriesA.map((e) => new Date(e.timestamp).getTime()));
      const maxTsB = Math.max(...entriesB.map((e) => new Date(e.timestamp).getTime()));
      if (maxTsB !== maxTsA) return maxTsB - maxTsA;
      return (a || '').localeCompare(b || '', undefined, { sensitivity: 'base' });
    });
    tlLanes = sortedFiles.map((f) => ({ file: f, entries: byFile.get(f) }));
    tlScrollY = 0;
    tlActiveNode = null;
    tlMode = 'multi';
    tlViewStart = viewStart;
    tlViewEnd = viewEnd;

    updateTimelineLabel();
    updateLaneLabels();
    requestTimelineDraw();
  }

  function updateTimelineLabel() {
    if (tlLanes.length === 0) {
      $timelineLabel.textContent = t('state.noActivity');
      return;
    }
    const s = new Date(tlViewStart);
    const e = new Date(tlViewEnd);
    $timelineLabel.textContent = formatDateTime(s) + ' \u2014 ' + formatDateTime(e);
  }

  /** Return indices into tlLanes that match tlLaneFilterQuery (empty = all). */
  function getVisibleLaneIndices() {
    const matcher = createFilterMatcher(tlLaneFilterQuery);
    if (!matcher) {
      return tlLanes.map((_, i) => i);
    }
    const out = [];
    for (let i = 0; i < tlLanes.length; i++) {
      if (matcher(tlLanes[i].file)) out.push(i);
    }
    return out;
  }

  /** Fit timeline view range to the time span of the current (filtered) file list. */
  function fitViewToCurrentFileList() {
    const visibleIndices = getVisibleLaneIndices();
    const entries = [];
    for (const i of visibleIndices) {
      if (tlLanes[i] && tlLanes[i].entries) {
        for (const e of tlLanes[i].entries) entries.push(e);
      }
    }
    if (entries.length === 0) return;
    let minTs = Infinity;
    let maxTs = -Infinity;
    for (const e of entries) {
      const ts = new Date(e.timestamp).getTime();
      if (ts < minTs) minTs = ts;
      if (ts > maxTs) maxTs = ts;
    }
    const span = Math.max(maxTs - minTs, MIN_VIEW_SPAN);
    const pad = span * 0.08;
    tlViewStart = minTs - pad;
    tlViewEnd = maxTs + pad;
    clearActiveRangeBtn();
    updateTimelineLabel();
    requestTimelineDraw();
  }

  function updateLaneLabels() {
    let spacer = $timelineLanes.querySelector('.tl-lane-spacer');
    if (!spacer) {
      spacer = document.createElement('div');
      spacer.className = 'tl-lane-spacer';
      spacer.style.height = RULER_HEIGHT + 'px';
      spacer.style.flexShrink = '0';
      const wrap = document.createElement('div');
      wrap.className = 'filter-wrap';
      const input = document.createElement('input');
      input.type = 'text';
      input.className = 'tl-lane-filter-input';
      input.placeholder = t('timeline.filterPlaceholder');
      input.value = tlLaneFilterQuery;
      input.setAttribute('aria-label', t('timeline.filterLanes'));
      let laneFilterTimeout = null;
      input.addEventListener('input', () => {
        tlLaneFilterQuery = input.value;
        clearTimeout(laneFilterTimeout);
        laneFilterTimeout = setTimeout(() => {
          updateLaneLabels();
          requestTimelineDraw();
        }, 80);
      });
      const clearBtn = document.createElement('button');
      clearBtn.type = 'button';
      clearBtn.className = 'filter-clear';
      clearBtn.setAttribute('aria-label', t('sidebar.clearFilter'));
      clearBtn.title = t('sidebar.clearTitle');
      clearBtn.textContent = '\u00D7';
      clearBtn.addEventListener('click', () => {
        input.value = '';
        tlLaneFilterQuery = '';
        updateLaneLabels();
        requestTimelineDraw();
        input.focus();
      });
      wrap.appendChild(input);
      wrap.appendChild(clearBtn);
      spacer.appendChild(wrap);
      $timelineLanes.appendChild(spacer);
    } else {
      const input = spacer.querySelector('.tl-lane-filter-input');
      if (input && input.value !== tlLaneFilterQuery) input.value = tlLaneFilterQuery;
    }

    while ($timelineLanes.lastChild !== spacer) {
      $timelineLanes.removeChild($timelineLanes.lastChild);
    }

    const visibleIndices = getVisibleLaneIndices();
    // Clamp tlScrollY to filtered content height so left list and canvas stay in sync
    const scrollContentHeight = visibleIndices.length * LANE_HEIGHT;
    const visibleHeight = $timelineLanes.clientHeight || 0;
    const maxScrollY = Math.max(0, scrollContentHeight - visibleHeight);
    tlScrollY = Math.min(tlScrollY, maxScrollY);

    for (let vi = 0; vi < visibleIndices.length; vi++) {
      const i = visibleIndices[vi];
      const lane = tlLanes[i];
      const label = document.createElement('div');
      label.className = 'tl-lane-label' + (currentFile === lane.file ? ' active' : '');
      // Show only the filename (last segment)
      const parts = lane.file.split('/');
      label.textContent = parts[parts.length - 1];
      label.title = lane.file;
      label.style.height = LANE_HEIGHT + 'px';
      label.style.lineHeight = LANE_HEIGHT + 'px';
      const file = lane.file;
      label.addEventListener('click', () => {
        const laneIdx = tlLanes.findIndex((l) => l.file === file);
        if (laneIdx === -1) return;
        const refTime =
          tlActiveNode != null
            ? new Date(tlActiveNode.entry.timestamp).getTime()
            : (tlViewStart + tlViewEnd) / 2;
        const node = findNodeInLaneClosestToTime(laneIdx, refTime);
        if (node) {
          onNodeClick(node);
          updateLaneLabels();
          requestAnimationFrame(() => {
            requestAnimationFrame(() => flashTooltipAtActiveNode());
          });
        }
      });
      $timelineLanes.appendChild(label);
    }
    // Sync scroll
    $timelineLanes.scrollTop = tlScrollY;
  }

  function clearTimelineLaneFilter() {
    tlLaneFilterQuery = '';
    updateLaneLabels();
    requestTimelineDraw();
  }

  // ---- Canvas drawing -------------------------------------------------------
  function resizeCanvas() {
    // Use the actual rendered size of the canvas element (flex:1 fills remaining space)
    const rect = $canvas.getBoundingClientRect();
    const w = rect.width;
    const h = rect.height;
    const dpr = window.devicePixelRatio || 1;
    $canvas.width = Math.round(w * dpr);
    $canvas.height = Math.round(h * dpr);
    $canvas.style.width = w + 'px';
    $canvas.style.height = h + 'px';
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  }

  function requestTimelineDraw() {
    if (tlRafId) return;
    tlRafId = requestAnimationFrame(() => {
      tlRafId = null;
      drawTimeline();
    });
  }

  function drawTimeline() {
    resizeCanvas();
    const w = parseFloat($canvas.style.width);
    const h = parseFloat($canvas.style.height);
    if (w <= 0 || h <= 0) return;

    ctx.clearRect(0, 0, w, h);

    drawRuler(w);
    drawDateSeparators(w, h);
    drawLanes(w, h);
  }

  /** Time-to-pixel conversion for the canvas drawing area */
  function timeToX(ts, canvasWidth) {
    if (tlViewEnd <= tlViewStart) return 0;
    return ((ts - tlViewStart) / (tlViewEnd - tlViewStart)) * canvasWidth;
  }

  function xToTime(x, canvasWidth) {
    if (canvasWidth <= 0) return tlViewStart;
    return tlViewStart + (x / canvasWidth) * (tlViewEnd - tlViewStart);
  }

  /** Pan/scroll the timeline so the active node stays within the visible canvas. */
  function ensureActiveNodeInView() {
    if (!tlActiveNode || !$canvas) return;
    const rect = $canvas.getBoundingClientRect();
    const w = rect.width;
    const h = rect.height;
    if (w <= 0 || h <= 0) return;
    const visibleIndices = getVisibleLaneIndices();
    const vi = visibleIndices.indexOf(tlActiveNode.laneIdx);
    if (vi === -1) return;
    const span = tlViewEnd - tlViewStart;
    if (span <= 0) return;

    const lanesAreaTop = RULER_HEIGHT;
    const lanesAreaHeight = h - lanesAreaTop;
    const ts = new Date(tlActiveNode.entry.timestamp).getTime();
    const nodeX = timeToX(ts, w);
    const laneY = lanesAreaTop + vi * LANE_HEIGHT - tlScrollY;
    const nodeCenterY = laneY + LANE_HEIGHT / 2;

    const margin = FOCUS_MARGIN;

    if (nodeX < margin) {
      const shift = ((margin - nodeX) / w) * span;
      tlViewStart -= shift;
      tlViewEnd -= shift;
    } else if (nodeX > w - margin) {
      const shift = ((nodeX - (w - margin)) / w) * span;
      tlViewStart += shift;
      tlViewEnd += shift;
    }

    const maxScrollY = Math.max(0, visibleIndices.length * LANE_HEIGHT - lanesAreaHeight);
    if (nodeCenterY < lanesAreaTop + margin) {
      tlScrollY = vi * LANE_HEIGHT + LANE_HEIGHT / 2 - margin;
    } else if (nodeCenterY > lanesAreaTop + lanesAreaHeight - margin) {
      tlScrollY = vi * LANE_HEIGHT + LANE_HEIGHT / 2 - (lanesAreaHeight - margin);
    }
    tlScrollY = Math.max(0, Math.min(maxScrollY, tlScrollY));
    if ($timelineLanes) $timelineLanes.scrollTop = tlScrollY;
    updateTimelineLabel();
  }

  /** Midnight (00:00) in local time for given timestamp */
  function getMidnightLocal(ts) {
    const d = new Date(ts);
    return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  }

  /** Draw vertical lines at 00:00 for each day in the visible range */
  function drawDateSeparators(w, h) {
    if (tlViewEnd <= tlViewStart) return;
    const MS_PER_DAY = 86400 * 1000;
    let midnight = getMidnightLocal(tlViewStart);
    if (midnight <= tlViewStart) midnight += MS_PER_DAY;

    ctx.save();
    ctx.strokeStyle = 'rgba(255,255,255,0.12)';
    ctx.lineWidth = 1;
    for (let t = midnight; t < tlViewEnd; t += MS_PER_DAY) {
      const x = timeToX(t, w);
      if (x < 0 || x > w) continue;
      ctx.beginPath();
      ctx.moveTo(Math.round(x) + 0.5, 0);
      ctx.lineTo(Math.round(x) + 0.5, h);
      ctx.stroke();
    }
    ctx.restore();
  }

  /** Draw the time ruler at the top */
  function drawRuler(w) {
    const span = tlViewEnd - tlViewStart;
    if (span <= 0) return;

    ctx.save();

    // Background
    ctx.fillStyle = COLORS.bgSurface;
    ctx.fillRect(0, 0, w, RULER_HEIGHT);

    // Bottom border
    ctx.strokeStyle = COLORS.border;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(0, RULER_HEIGHT - 0.5);
    ctx.lineTo(w, RULER_HEIGHT - 0.5);
    ctx.stroke();

    // Choose tick interval (smallest first so zoom-in picks second-level)
    const tickIntervals = [
      { ms: 15 * 1000, label: '15s' }, // 15 sec
      { ms: 30 * 1000, label: '30s' }, // 30 sec
      { ms: 60 * 1000, label: 'min' }, // 1 min
      { ms: 5 * 60 * 1000, label: '5min' }, // 5 min
      { ms: 15 * 60 * 1000, label: '15min' }, // 15 min
      { ms: 30 * 60 * 1000, label: '30min' }, // 30 min
      { ms: 3600 * 1000, label: 'hour' }, // 1 hour
      { ms: 3 * 3600 * 1000, label: '3h' }, // 3 hours
      { ms: 6 * 3600 * 1000, label: '6h' }, // 6 hours
      { ms: 12 * 3600 * 1000, label: '12h' }, // 12 hours
      { ms: 86400 * 1000, label: 'day' }, // 1 day
      { ms: 7 * 86400 * 1000, label: 'week' }, // 1 week
      { ms: 30 * 86400 * 1000, label: 'month' }, // ~1 month
    ];

    // Target about 6-12 ticks across the width
    const targetTickCount = 8;
    const idealInterval = span / targetTickCount;
    let tickMs = tickIntervals[tickIntervals.length - 1].ms;
    let tickType = tickIntervals[tickIntervals.length - 1].label;
    for (const ti of tickIntervals) {
      if (ti.ms >= idealInterval * 0.5) {
        tickMs = ti.ms;
        tickType = ti.label;
        break;
      }
    }

    // Draw ticks
    const firstTick = Math.ceil(tlViewStart / tickMs) * tickMs;
    ctx.fillStyle = COLORS.fgDim;
    ctx.font = '10px -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif';
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';

    for (let t = firstTick; t <= tlViewEnd; t += tickMs) {
      const x = timeToX(t, w);
      if (x < 0 || x > w) continue;

      // Tick line
      ctx.strokeStyle = COLORS.border;
      ctx.beginPath();
      ctx.moveTo(Math.round(x) + 0.5, RULER_HEIGHT - 6);
      ctx.lineTo(Math.round(x) + 0.5, RULER_HEIGHT - 1);
      ctx.stroke();

      // Label
      const d = new Date(t);
      let label;
      if (tickType === 'day' || tickType === 'week' || tickType === 'month') {
        label =
          (d.getMonth() + 1).toString().padStart(2, '0') +
          '/' +
          d.getDate().toString().padStart(2, '0');
      } else if (tickType === '15s' || tickType === '30s') {
        label =
          d.getHours().toString().padStart(2, '0') +
          ':' +
          d.getMinutes().toString().padStart(2, '0') +
          ':' +
          d.getSeconds().toString().padStart(2, '0');
      } else {
        label =
          d.getHours().toString().padStart(2, '0') +
          ':' +
          d.getMinutes().toString().padStart(2, '0');
      }
      ctx.fillText(label, x, RULER_HEIGHT / 2);
    }

    ctx.restore();
  }

  /** Draw lanes and event nodes */
  function drawLanes(w, h) {
    ctx.save();

    const lanesAreaTop = RULER_HEIGHT;
    const lanesAreaHeight = h - RULER_HEIGHT;
    const visibleIndices = getVisibleLaneIndices();

    // Clip to lanes area
    ctx.beginPath();
    ctx.rect(0, lanesAreaTop, w, lanesAreaHeight);
    ctx.clip();

    for (let vi = 0; vi < visibleIndices.length; vi++) {
      const li = visibleIndices[vi];
      const lane = tlLanes[li];
      const laneY = lanesAreaTop + vi * LANE_HEIGHT - tlScrollY;

      // Skip if not visible
      if (laneY + LANE_HEIGHT < lanesAreaTop || laneY > h) continue;

      // Lane background - subtle alternating
      if (vi % 2 === 1) {
        ctx.fillStyle = 'rgba(255,255,255,0.02)';
        ctx.fillRect(0, laneY, w, LANE_HEIGHT);
      }

      // Lane bottom border
      ctx.strokeStyle = 'rgba(255,255,255,0.04)';
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(0, laneY + LANE_HEIGHT - 0.5);
      ctx.lineTo(w, laneY + LANE_HEIGHT - 0.5);
      ctx.stroke();

      // Draw connection lines between adjacent nodes
      const centerY = laneY + LANE_HEIGHT / 2;
      const visibleEntries = [];
      for (let ei = 0; ei < lane.entries.length; ei++) {
        const entry = lane.entries[ei];
        const ts = new Date(entry.timestamp).getTime();
        const x = timeToX(ts, w);
        visibleEntries.push({ x, ei, entry });
      }

      // Draw connecting line segments
      if (visibleEntries.length > 1) {
        ctx.strokeStyle = COLORS.border;
        ctx.lineWidth = 1.5;
        ctx.beginPath();
        ctx.moveTo(visibleEntries[0].x, centerY);
        for (let i = 1; i < visibleEntries.length; i++) {
          ctx.lineTo(visibleEntries[i].x, centerY);
        }
        ctx.stroke();
      }

      // Draw nodes (skip the active node so we can draw it on top later)
      for (const ve of visibleEntries) {
        const { x, ei, entry } = ve;
        if (x < -NODE_RADIUS * 2 || x > w + NODE_RADIUS * 2) continue;
        if (tlActiveNode && tlActiveNode.laneIdx === li && tlActiveNode.entryIdx === ei) continue;

        const color = opColor(entry.op);
        const isHovered =
          tlHoveredNode && tlHoveredNode.laneIdx === li && tlHoveredNode.entryIdx === ei;

        ctx.beginPath();
        ctx.arc(x, centerY, isHovered ? NODE_RADIUS + 1.5 : NODE_RADIUS, 0, Math.PI * 2);
        ctx.fillStyle = COLORS.bg;
        ctx.fill();
        ctx.strokeStyle = color;
        ctx.lineWidth = 2;
        ctx.stroke();
      }
    }

    // Draw the active node on top so it is never covered by others
    if (tlActiveNode) {
      const li = tlActiveNode.laneIdx;
      const vi = visibleIndices.indexOf(li);
      if (vi === -1) {
        // Active lane filtered out - skip drawing
      } else {
        const laneY = lanesAreaTop + vi * LANE_HEIGHT - tlScrollY;
        if (laneY + LANE_HEIGHT >= lanesAreaTop && laneY <= h) {
          const centerY = laneY + LANE_HEIGHT / 2;
          const entry = tlActiveNode.entry;
          const ts = new Date(entry.timestamp).getTime();
          const x = timeToX(ts, w);
          if (x >= -NODE_RADIUS * 2 && x <= w + NODE_RADIUS * 2) {
            const color = opColor(entry.op);
            const isHovered =
              tlHoveredNode &&
              tlHoveredNode.laneIdx === li &&
              tlHoveredNode.entryIdx === tlActiveNode.entryIdx;

            ctx.beginPath();
            ctx.arc(x, centerY, isHovered ? NODE_RADIUS + 1.5 : NODE_RADIUS, 0, Math.PI * 2);
            ctx.fillStyle = color;
            ctx.fill();
            ctx.strokeStyle = COLORS.fgBright;
            ctx.lineWidth = 2;
            ctx.stroke();
            ctx.beginPath();
            ctx.arc(x, centerY, 2, 0, Math.PI * 2);
            ctx.fillStyle = COLORS.fgBright;
            ctx.fill();
          }
        }
      }
    }

    ctx.restore();
  }

  // ---- Canvas interactions --------------------------------------------------
  function getCanvasCoords(e) {
    const rect = $canvas.getBoundingClientRect();
    return { x: e.clientX - rect.left, y: e.clientY - rect.top };
  }

  function hitTestNode(mx, my) {
    const w = parseFloat($canvas.style.width);
    const lanesAreaTop = RULER_HEIGHT;
    const visibleIndices = getVisibleLaneIndices();
    let closest = null;
    let closestDist = NODE_HIT_RADIUS + 1;

    for (let vi = 0; vi < visibleIndices.length; vi++) {
      const li = visibleIndices[vi];
      const lane = tlLanes[li];
      const laneY = lanesAreaTop + vi * LANE_HEIGHT - tlScrollY;
      const centerY = laneY + LANE_HEIGHT / 2;

      for (let ei = 0; ei < lane.entries.length; ei++) {
        const entry = lane.entries[ei];
        const ts = new Date(entry.timestamp).getTime();
        const x = timeToX(ts, w);
        const dx = mx - x;
        const dy = my - centerY;
        const dist = Math.sqrt(dx * dx + dy * dy);

        if (dist < closestDist) {
          closestDist = dist;
          closest = { laneIdx: li, entryIdx: ei, entry, x, y: centerY };
        }
      }
    }

    return closest;
  }

  /** Find the node in a given lane closest in time to refTime (for lane label single-click) */
  function findNodeInLaneClosestToTime(laneIdx, refTimeMs) {
    if (laneIdx < 0 || laneIdx >= tlLanes.length) return null;
    const lane = tlLanes[laneIdx];
    if (lane.entries.length === 0) return null;
    const w = parseFloat($canvas.style.width);
    if (w <= 0) return null;
    let best = null;
    let bestDist = Infinity;
    for (let ei = 0; ei < lane.entries.length; ei++) {
      const entry = lane.entries[ei];
      const ts = new Date(entry.timestamp).getTime();
      const dist = Math.abs(ts - refTimeMs);
      if (dist < bestDist) {
        bestDist = dist;
        const x = timeToX(ts, w);
        const vi = getVisibleLaneIndices().indexOf(laneIdx);
        const lanesAreaTop = RULER_HEIGHT;
        const centerY = lanesAreaTop + vi * LANE_HEIGHT - tlScrollY + LANE_HEIGHT / 2;
        best = { laneIdx, entryIdx: ei, entry, x, y: centerY };
      }
    }
    return best;
  }

  function onCanvasPointerDown(e) {
    if (e.button !== 0) return;

    tlDragState = {
      startX: e.clientX,
      startY: e.clientY,
      origViewStart: tlViewStart,
      origViewEnd: tlViewEnd,
      origScrollY: tlScrollY,
      moved: false,
    };

    $canvas.classList.add('grabbing');
    document.body.classList.add('tl-dragging');
    $canvas.setPointerCapture(e.pointerId);
  }

  function onCanvasPointerMove(e) {
    const { x, y } = getCanvasCoords(e);

    if (tlDragState) {
      const dx = e.clientX - tlDragState.startX;
      const dy = e.clientY - tlDragState.startY;

      if (Math.abs(dx) > 3 || Math.abs(dy) > 3) {
        tlDragState.moved = true;
      }

      // Pan horizontally
      const w = parseFloat($canvas.style.width);
      if (w > 0) {
        const span = tlDragState.origViewEnd - tlDragState.origViewStart;
        const timeDelta = (dx / w) * span;
        tlViewStart = tlDragState.origViewStart - timeDelta;
        tlViewEnd = tlDragState.origViewEnd - timeDelta;
      }

      // Pan vertically (use visible/filtered lane count so scroll matches left list)
      const lanesAreaHeight = parseFloat($canvas.style.height) - RULER_HEIGHT;
      const visibleIndices = getVisibleLaneIndices();
      const maxScrollY = Math.max(0, visibleIndices.length * LANE_HEIGHT - lanesAreaHeight);
      tlScrollY = Math.max(0, Math.min(maxScrollY, tlDragState.origScrollY - dy));
      $timelineLanes.scrollTop = tlScrollY;

      updateTimelineLabel();
      requestTimelineDraw();
      hideTooltip();
    } else {
      // Hover hit test
      const node = hitTestNode(x, y);
      if (node) {
        tlHoveredNode = node;
        showTooltipAtNode(node);
      } else {
        if (tlHoveredNode) {
          tlHoveredNode = null;
          hideTooltip();
        }
      }
      requestTimelineDraw();
    }
  }

  function onCanvasPointerUp(e) {
    if (!tlDragState) return;
    const wasDrag = tlDragState.moved;
    tlDragState = null;
    $canvas.classList.remove('grabbing');
    document.body.classList.remove('tl-dragging');
    try {
      e.target.releasePointerCapture(e.pointerId);
    } catch {
      /* releasePointerCapture can throw if pointer was already released */
    }

    if (!wasDrag) {
      const { x, y } = getCanvasCoords(e);
      const node = hitTestNode(x, y);
      if (node) {
        onNodeClick(node);
        updateLaneLabels();
        requestAnimationFrame(() => {
          requestAnimationFrame(() => flashTooltipAtActiveNode());
        });
      }
    }
  }

  function onCanvasWheel(e) {
    e.preventDefault();
    const { x } = getCanvasCoords(e);
    const w = parseFloat($canvas.style.width);
    if (w <= 0) return;

    // Shift+wheel or trackpad horizontal: horizontal pan
    if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
      const span = tlViewEnd - tlViewStart;
      const delta = (e.deltaX || e.deltaY) > 0 ? 1 : -1;
      const panAmount = span * 0.015 * delta;
      tlViewStart += panAmount;
      tlViewEnd += panAmount;
    } else if (e.ctrlKey || e.metaKey) {
      // Ctrl/Cmd + wheel: zoom around mouse position (proportional to current range)
      let dy = e.deltaY;
      if (e.deltaMode === 1) dy *= 16; // line mode
      if (e.deltaMode === 2) dy *= 100; // page mode

      // Clamp to prevent extreme jumps from trackpad pinch or high-momentum scroll
      dy = Math.max(-100, Math.min(100, dy));

      const zoomIntensity = 0.0015;
      const factor = Math.exp(dy * zoomIntensity);

      const mouseTime = xToTime(x, w);
      const leftRatio = (mouseTime - tlViewStart) / (tlViewEnd - tlViewStart);

      const newSpan = (tlViewEnd - tlViewStart) * factor;

      tlViewStart = mouseTime - leftRatio * newSpan;
      tlViewEnd = mouseTime + (1 - leftRatio) * newSpan;
    } else {
      // Default wheel: vertical scroll lanes (use visible/filtered lane count)
      const lanesAreaHeight = parseFloat($canvas.style.height) - RULER_HEIGHT;
      const visibleIndices = getVisibleLaneIndices();
      const maxScrollY = Math.max(0, visibleIndices.length * LANE_HEIGHT - lanesAreaHeight);
      tlScrollY = Math.max(0, Math.min(maxScrollY, tlScrollY + e.deltaY));
      $timelineLanes.scrollTop = tlScrollY;
    }

    updateTimelineLabel();
    requestTimelineDraw();
  }

  function onNodeClick(node) {
    tlActiveNode = node;
    const lane = tlLanes[node.laneIdx];

    if (tlMode === 'multi' && lane.file !== currentFile) {
      // In multi-file mode, clicking a node switches to that file
      // but don't refetch - we set up single-file from existing data
      currentFile = lane.file;
      rememberSelectedFiles(Array.from(selectedFiles));
      renderFileList();
      updateLaneLabels();
      $diffTitle.textContent = lane.file;

      apiJson('/api/history?file=' + encodeURIComponent(lane.file)).then((entries) => {
        historyEntries = entries;
        const idx = findEntryIndex(entries, node.entry.timestamp, node.entry.checksum);
        selectEntryDiff(idx >= 0 ? idx : entries.length - 1);
      });
    } else {
      const matchIdx = findEntryIndex(historyEntries, node.entry.timestamp, node.entry.checksum);
      if (matchIdx >= 0) {
        selectEntryDiff(matchIdx);
      }
    }

    ensureActiveNodeInView();
    requestTimelineDraw();
  }

  function showTooltipAtNode(node) {
    const entry = node.entry;
    let text = opLabel(entry.op) + ' \u2022 ' + formatDateTime(new Date(entry.timestamp));
    if (entry.size != null) {
      text += ' \u2022 ' + formatSize(entry.size);
    }
    if (tlMode === 'multi') {
      text = tlLanes[node.laneIdx].file + '\n' + text;
    }
    $tlTooltip.textContent = text;
    $tlTooltip.style.whiteSpace = tlMode === 'multi' ? 'pre' : 'nowrap';
    $tlTooltip.classList.add('visible');

    // Compute the node's screen position in viewport coordinates (for fixed positioning)
    const canvasRect = $canvas.getBoundingClientRect();
    const w = parseFloat($canvas.style.width);
    const ts = new Date(entry.timestamp).getTime();
    const nodeX = timeToX(ts, w);
    const visibleIndices = getVisibleLaneIndices();
    const vi = visibleIndices.indexOf(node.laneIdx);
    if (vi === -1) return;
    const laneY = RULER_HEIGHT + vi * LANE_HEIGHT - tlScrollY;
    const nodeCenterY = laneY + LANE_HEIGHT / 2;

    // Viewport coords of the node center
    const vpX = canvasRect.left + nodeX;
    const vpY = canvasRect.top + nodeCenterY;

    // Measure tooltip after making visible
    const tooltipRect = $tlTooltip.getBoundingClientRect();
    let left = vpX - tooltipRect.width / 2;
    let top = vpY - NODE_RADIUS - tooltipRect.height - 6;

    // Clamp within viewport
    left = Math.max(4, Math.min(window.innerWidth - tooltipRect.width - 4, left));
    if (top < 0) top = vpY + NODE_RADIUS + 6;

    $tlTooltip.style.left = left + 'px';
    $tlTooltip.style.top = top + 'px';
  }

  /** Show tooltip briefly (for keyboard navigation) */
  function flashTooltipAtActiveNode() {
    if (!tlActiveNode || tlLanes.length === 0) return;
    showTooltipAtNode(tlActiveNode);
    clearTimeout(tlTooltipTimeoutId);
    tlTooltipTimeoutId = setTimeout(hideTooltip, 1200);
  }

  function hideTooltip() {
    $tlTooltip.classList.remove('visible');
    clearTimeout(tlTooltipTimeoutId);
  }

  /** Select an entry for diff viewing (updates diff panel without touching timeline) */
  async function selectEntryDiff(idx) {
    activeEntryIdx = idx;
    const entry = historyEntries[idx];
    selectedRestoreChecksum = entry && entry.checksum ? entry.checksum : null;
    updateRestoreButton();

    // Update timeline active node to match
    if (tlLanes.length > 0) {
      const matched = matchActiveNodeForEntry(entry);
      if (matched) {
        tlActiveNode = matched;
      }
      requestTimelineDraw();
    }

    // Build diff query
    const toChecksum = entry.checksum;

    if (!toChecksum) {
      // Delete event - show message
      $diffViewer.innerHTML =
        '<div class="empty-state">' + escapeHtml(t('state.fileDeleted')) + '</div>';
      $diffMeta.textContent =
        opLabel(entry.op) + ' \u2022 ' + formatDateTime(new Date(entry.timestamp));
      return;
    }

    // Find previous checksum
    let fromChecksum = null;
    for (let i = idx - 1; i >= 0; i--) {
      const prev = historyEntries[i];
      if (!prev.checksum) {
        break;
      }
      fromChecksum = prev.checksum;
      break;
    }

    $diffMeta.textContent =
      opLabel(entry.op) + ' \u2022 ' + formatDateTime(new Date(entry.timestamp));
    if (entry.size != null) {
      $diffMeta.textContent += ' \u2022 ' + formatSize(entry.size);
    }

    $diffViewer.innerHTML =
      '<div class="loading">' + escapeHtml(t('state.computingDiff')) + '</div>';

    try {
      let url =
        '/api/diff?file=' +
        encodeURIComponent(currentFile) +
        '&to=' +
        encodeURIComponent(toChecksum);
      if (fromChecksum) {
        url += '&from=' + encodeURIComponent(fromChecksum);
      }
      const diff = await apiJson(url);
      renderDiff(diff, fromChecksum, toChecksum);
    } catch (e) {
      $diffViewer.innerHTML = '<div class="empty-state">' + escapeHtml(e.message) + '</div>';
    }
  }

  function matchActiveNodeForEntry(entry) {
    if (!entry || tlLanes.length === 0) return null;
    if (tlMode === 'single') {
      const ei = findEntryIndex(tlLanes[0].entries, entry.timestamp, entry.checksum);
      if (ei >= 0) return { laneIdx: 0, entryIdx: ei, entry: tlLanes[0].entries[ei] };
      return null;
    }
    for (let li = 0; li < tlLanes.length; li++) {
      if (tlLanes[li].file !== currentFile) continue;
      const ei = findEntryIndex(tlLanes[li].entries, entry.timestamp, entry.checksum);
      if (ei >= 0) return { laneIdx: li, entryIdx: ei, entry: tlLanes[li].entries[ei] };
      break;
    }
    return null;
  }

  /** Legacy selectEntry that also works with the new timeline */
  async function selectEntry(idx, showFlash) {
    if (tlLanes.length > 0 && idx >= 0 && idx < historyEntries.length) {
      const matched = matchActiveNodeForEntry(historyEntries[idx]);
      if (matched) {
        tlActiveNode = matched;
        ensureActiveNodeInView();
      }
      requestTimelineDraw();
      if (showFlash) {
        // Wait for draw rAF to finish, then flash tooltip on the next frame
        requestAnimationFrame(() => {
          requestAnimationFrame(() => flashTooltipAtActiveNode());
        });
      }
    }
    await selectEntryDiff(idx);
  }

  // ---- Range buttons --------------------------------------------------------
  function clearActiveRangeBtn() {
    if (tlActiveRangeBtn) {
      localStorage.removeItem(TIMELINE_RANGE_STORAGE_KEY);
      tlActiveRangeBtn.classList.remove('active');
      tlActiveRangeBtn = null;
    }
  }

  function initRangeButtons() {
    const buttons = document.querySelectorAll('.tl-range-btn');
    buttons.forEach((btn) => {
      btn.addEventListener('click', () => onRangeButtonClick(btn));
    });
  }

  async function onRangeButtonClick(btn) {
    const range = btn.dataset.range;

    // Toggle off if same button clicked
    if (tlActiveRangeBtn === btn) {
      clearActiveRangeBtn();
      if (currentFile) {
        tlMode = 'single';
        setTimelineSingleFile(currentFile, shouldShowOnTimeline() ? historyEntries : []);
      }
      return;
    }

    clearActiveRangeBtn();
    btn.classList.add('active');
    tlActiveRangeBtn = btn;

    const now = Date.now();
    let since;
    let until = now;

    if (range === 'all') {
      // Load all activity - use a very old date
      since = 0;
    } else {
      const ms = parseInt(range, 10);
      since = now - ms;
    }

    try {
      const sinceISO = new Date(since).toISOString();
      const untilISO = new Date(until).toISOString();
      const includeDeleted = !hideDeletedFiles;
      const entries = await apiJson(
        '/api/activity?since=' +
          encodeURIComponent(sinceISO) +
          '&until=' +
          encodeURIComponent(untilISO) +
          '&include_deleted=' +
          includeDeleted
      );

      if (entries.length === 0) {
        tlMode = 'multi';
        tlLanes = [];
        tlViewStart = since || now - 3600000;
        tlViewEnd = until;
        updateTimelineLabel();
        updateLaneLabels();
        requestTimelineDraw();
        localStorage.setItem(TIMELINE_RANGE_STORAGE_KEY, range);
        return;
      }

      // For "all" range, derive actual time range from entries
      if (range === 'all') {
        const first = new Date(entries[0].timestamp).getTime();
        const last = new Date(entries[entries.length - 1].timestamp).getTime();
        const span = Math.max(last - first, MIN_VIEW_SPAN);
        const pad = span * 0.08;
        since = first - pad;
        until = last + pad;
      }

      setTimelineMultiFile(entries, since, until);
      localStorage.setItem(TIMELINE_RANGE_STORAGE_KEY, range);
      if (tlLanes.length > 0) {
        currentFile = tlLanes[0].file;
        renderFileList();
        updateLaneLabels();
        const lane = tlLanes[0];
        if (lane.entries.length > 0) {
          const lastEntry = lane.entries[lane.entries.length - 1];
          apiJson('/api/history?file=' + encodeURIComponent(currentFile)).then((hist) => {
            historyEntries = hist;
            const idx = findEntryIndex(hist, lastEntry.timestamp, lastEntry.checksum);
            const matchIdx = idx >= 0 ? idx : hist.length - 1;
            tlActiveNode = {
              laneIdx: 0,
              entryIdx: lane.entries.length - 1,
              entry: lastEntry,
            };
            $diffTitle.textContent = currentFile;
            selectEntryDiff(matchIdx);
            requestTimelineDraw();
          });
        } else {
          requestTimelineDraw();
        }
      }
    } catch (e) {
      $status.textContent = t('status.activityFailed', { msg: e.message });
    }
  }

  // ---- Initialize timeline event listeners ----------------------------------
  function initTimeline() {
    $canvas.addEventListener('pointerdown', onCanvasPointerDown);
    $canvas.addEventListener('pointermove', onCanvasPointerMove);
    $canvas.addEventListener('pointerup', onCanvasPointerUp);
    $canvas.addEventListener('pointerleave', () => {
      if (tlHoveredNode) {
        tlHoveredNode = null;
        hideTooltip();
        requestTimelineDraw();
      }
    });
    $canvas.addEventListener('wheel', onCanvasWheel, { passive: false });

    // Sync vertical scroll between lanes and canvas
    $timelineLanes.addEventListener('scroll', () => {
      tlScrollY = $timelineLanes.scrollTop;
      requestTimelineDraw();
    });

    // Redraw on resize
    const resizeObserver = new ResizeObserver(() => {
      requestTimelineDraw();
    });
    resizeObserver.observe($timelineBody);

    // Track mouse position for keyboard navigation context
    $timelineBar.addEventListener('mouseenter', () => {
      isMouseOverTimeline = true;
    });
    $timelineBar.addEventListener('mouseleave', () => {
      isMouseOverTimeline = false;
    });

    initRangeButtons();
    initDragResize({
      handle: $tlResizeHandle,
      target: $timelineBar,
      storageKey: TL_HEIGHT_STORAGE_KEY,
      axis: 'y',
      sizeProp: 'height',
      minSize: 60,
      invert: true,
      getMaxSize: function () {
        return window.innerHeight * 0.5;
      },
      bodyClass: 'tl-resizing',
      onResize: requestTimelineDraw,
    });
    initDragResize({
      handle: $tlLanesResize,
      target: $timelineLanes,
      storageKey: TL_LANES_WIDTH_STORAGE_KEY,
      axis: 'x',
      sizeProp: 'width',
      minSize: 60,
      getMaxSize: function () {
        return $timelineBody.getBoundingClientRect().width * 0.4;
      },
      bodyClass: 'tl-lanes-resizing',
      onResize: requestTimelineDraw,
    });
  }

  // ---- Diff renderer -------------------------------------------------------
  // Uses a virtual-scroll approach: we build a flat list of row descriptors and
  // only materialize DOM rows inside the visible viewport.

  const ROW_HEIGHT = 20; // must match CSS line-height
  const OVERSCAN = 20; // extra rows above/below viewport

  function renderDiff(diff, fromChecksum, toChecksum) {
    diffRows = [];
    lastDiffFromChecksum = fromChecksum || '';
    lastDiffToChecksum = toChecksum || '';
    diffSingleMode = !fromChecksum;

    if (diff.hunks.length === 0) {
      $diffViewer.innerHTML =
        '<div class="empty-state">' + escapeHtml(t('state.noChanges')) + '</div>';
      return;
    }

    // Build flat row list with separators between hunks
    for (let hi = 0; hi < diff.hunks.length; hi++) {
      const hunk = diff.hunks[hi];

      // Separator before hunk (skip for the very first if it starts at line 1)
      if (hi > 0 || hunk.old_start > 1 || hunk.new_start > 1) {
        let skippedOld;
        let skippedNew;
        let oldFrom = 1;
        let oldTo = hunk.old_start - 1;
        let newFrom = 1;
        let newTo = hunk.new_start - 1;
        if (hi === 0) {
          skippedOld = hunk.old_start - 1;
          skippedNew = hunk.new_start - 1;
        } else {
          const prev = diff.hunks[hi - 1];
          const prevEndOld = hunkEndLine(prev, 'old');
          const prevEndNew = hunkEndLine(prev, 'new');
          skippedOld = hunk.old_start - prevEndOld;
          skippedNew = hunk.new_start - prevEndNew;
          oldFrom = prevEndOld;
          oldTo = hunk.old_start - 1;
          newFrom = prevEndNew;
          newTo = hunk.new_start - 1;
        }
        const skipped = Math.max(skippedOld, skippedNew);
        if (skipped > 0) {
          diffRows.push({
            type: 'separator',
            text: t('diff.unchangedLines', { n: skipped }),
            oldFrom,
            oldTo,
            newFrom,
            newTo,
          });
        }
      }

      // Lines
      let oldLine = hunk.old_start;
      let newLine = hunk.new_start;
      for (const line of hunk.lines) {
        const row = {
          type: line.tag,
          content: line.content,
          oldNum: null,
          newNum: null,
        };
        if (line.tag === 'equal') {
          row.oldNum = oldLine++;
          row.newNum = newLine++;
        } else if (line.tag === 'delete') {
          row.oldNum = oldLine++;
        } else if (line.tag === 'insert') {
          row.newNum = newLine++;
        }
        diffRows.push(row);
      }
    }

    // Trailing separator
    const lastHunk = diff.hunks[diff.hunks.length - 1];
    const lastEndOld = hunkEndLine(lastHunk, 'old');
    const lastEndNew = hunkEndLine(lastHunk, 'new');
    if (lastEndOld <= diff.old_total || lastEndNew <= diff.new_total) {
      const remaining = Math.max(diff.old_total - lastEndOld + 1, diff.new_total - lastEndNew + 1);
      if (remaining > 0) {
        diffRows.push({
          type: 'separator',
          text: t('diff.unchangedLines', { n: remaining }),
          oldFrom: lastEndOld,
          oldTo: diff.old_total,
          newFrom: lastEndNew,
          newTo: diff.new_total,
        });
      }
    }

    initVirtualScroll();
  }

  function hunkEndLine(hunk, side) {
    let line = side === 'old' ? hunk.old_start : hunk.new_start;
    const incTag = side === 'old' ? 'delete' : 'insert';
    for (const l of hunk.lines) {
      if (l.tag === 'equal' || l.tag === incTag) line++;
    }
    return line;
  }

  // ---- Virtual scroll ------------------------------------------------------
  let vsContainer = null; // the scrollable wrapper
  let vsSpacer = null; // tall empty div for scroll height
  let vsContent = null; // positioned table holding visible rows
  let vsBody = null; // tbody for diff rows
  let vsRafId = null;

  function initVirtualScroll() {
    $diffViewer.innerHTML = '';

    vsContainer = $diffViewer;
    vsSpacer = document.createElement('div');
    vsSpacer.style.height = diffRows.length * ROW_HEIGHT + 'px';
    vsSpacer.style.position = 'relative';

    vsContent = document.createElement('table');
    vsContent.className = 'diff-table' + (diffSingleMode ? ' diff-single' : '');
    vsContent.style.position = 'absolute';
    vsContent.style.left = '0';
    vsContent.style.right = '0';
    vsContent.style.willChange = 'transform';

    const colgroup = document.createElement('colgroup');
    const colGutterOld = document.createElement('col');
    colGutterOld.className = 'diff-col-gutter diff-col-gutter-old';
    const colCodeOld = document.createElement('col');
    colCodeOld.className = 'diff-col-code diff-col-code-old';
    const colGutterNew = document.createElement('col');
    colGutterNew.className = 'diff-col-gutter diff-col-gutter-new';
    const colCodeNew = document.createElement('col');
    colCodeNew.className = 'diff-col-code diff-col-code-new';
    colgroup.appendChild(colGutterOld);
    colgroup.appendChild(colCodeOld);
    colgroup.appendChild(colGutterNew);
    colgroup.appendChild(colCodeNew);
    vsContent.appendChild(colgroup);

    vsBody = document.createElement('tbody');
    vsContent.appendChild(vsBody);

    vsSpacer.appendChild(vsContent);
    vsContainer.appendChild(vsSpacer);

    vsContainer.addEventListener('scroll', onVsScroll, { passive: true });
    renderVisibleRows();
  }

  function onVsScroll() {
    if (vsRafId) return;
    vsRafId = requestAnimationFrame(() => {
      vsRafId = null;
      renderVisibleRows();
    });
  }

  function renderVisibleRows() {
    if (!vsContainer || diffRows.length === 0) return;

    const scrollTop = vsContainer.scrollTop;
    const viewHeight = vsContainer.clientHeight;

    let startIdx = Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN;
    let endIdx = Math.ceil((scrollTop + viewHeight) / ROW_HEIGHT) + OVERSCAN;
    startIdx = Math.max(0, startIdx);
    endIdx = Math.min(diffRows.length, endIdx);

    vsContent.style.top = startIdx * ROW_HEIGHT + 'px';

    // Build rows
    const showOld = !diffSingleMode;
    const frag = document.createDocumentFragment();
    for (let i = startIdx; i < endIdx; i++) {
      const row = diffRows[i];
      const tr = document.createElement('tr');
      tr.style.height = ROW_HEIGHT + 'px';

      if (row.type === 'separator') {
        tr.className = 'diff-separator';
        const td = document.createElement('td');
        td.colSpan = showOld ? 4 : 2;
        td.textContent = row.text;
        td.addEventListener('click', () => expandSeparator(i));
        tr.appendChild(td);
      } else {
        tr.className = 'diff-line-' + row.type;
        const gutterNew = document.createElement('td');
        gutterNew.className = 'diff-gutter diff-gutter-new';
        gutterNew.textContent = row.newNum != null ? row.newNum : '';

        const codeNew = document.createElement('td');
        codeNew.className = 'diff-code diff-code-new';

        if (showOld) {
          const gutterOld = document.createElement('td');
          gutterOld.className = 'diff-gutter diff-gutter-old';
          gutterOld.textContent = row.oldNum != null ? row.oldNum : '';

          const codeOld = document.createElement('td');
          codeOld.className = 'diff-code diff-code-old';

          if (row.type === 'equal') {
            codeOld.textContent = row.content;
            codeNew.textContent = row.content;
          } else if (row.type === 'delete') {
            codeOld.textContent = row.content;
            codeNew.textContent = '';
          } else if (row.type === 'insert') {
            codeOld.textContent = '';
            codeNew.textContent = row.content;
          }

          tr.appendChild(gutterOld);
          tr.appendChild(codeOld);
        } else {
          if (row.type === 'delete') {
            codeNew.textContent = '';
          } else {
            codeNew.textContent = row.content;
          }
        }

        tr.appendChild(gutterNew);
        tr.appendChild(codeNew);
      }

      frag.appendChild(tr);
    }

    vsBody.innerHTML = '';
    vsBody.appendChild(frag);
  }

  async function expandSeparator(rowIdx) {
    const row = diffRows[rowIdx];
    if (!row || row.type !== 'separator') return;
    if (!lastDiffToChecksum) return;

    const oldFrom = row.oldFrom || 0;
    const oldTo = row.oldTo || 0;
    const newFrom = row.newFrom || 0;
    const newTo = row.newTo || 0;

    if (oldTo < oldFrom && newTo < newFrom) return;

    try {
      const [oldText, newText] = await Promise.all([
        lastDiffFromChecksum
          ? apiText('/api/snapshot?checksum=' + encodeURIComponent(lastDiffFromChecksum))
          : Promise.resolve(''),
        apiText('/api/snapshot?checksum=' + encodeURIComponent(lastDiffToChecksum)),
      ]);

      const oldLines = normalizeLines(oldText);
      const newLines = normalizeLines(newText);

      const oldSlice = oldTo >= oldFrom ? oldLines.slice(oldFrom - 1, oldTo) : [];
      const newSlice = newTo >= newFrom ? newLines.slice(newFrom - 1, newTo) : [];
      const count = Math.max(oldSlice.length, newSlice.length);

      const expanded = [];
      let oldLineNum = oldFrom;
      let newLineNum = newFrom;
      for (let i = 0; i < count; i++) {
        const oldContent = oldSlice[i] ?? '';
        const newContent = newSlice[i] ?? '';
        expanded.push({
          type: 'equal',
          content: oldContent || newContent,
          oldNum: oldSlice[i] != null ? oldLineNum++ : null,
          newNum: newSlice[i] != null ? newLineNum++ : null,
        });
      }

      diffRows.splice(rowIdx, 1, ...expanded);
      if (vsSpacer) {
        vsSpacer.style.height = diffRows.length * ROW_HEIGHT + 'px';
      }
      renderVisibleRows();
    } catch {
      row.text = t('state.failedSnapshot');
      renderVisibleRows();
    }
  }

  // ---- Scan button ---------------------------------------------------------
  $btnScan.addEventListener('click', async () => {
    $btnScan.disabled = true;
    $btnScan.textContent = t('toolbar.scanning');
    try {
      const result = await apiPost('/api/scan');
      $status.textContent = t('status.scanResult', {
        created: result.created,
        modified: result.modified,
        deleted: result.deleted,
      });
      $btnScan.disabled = false;
      $btnScan.textContent = t('toolbar.scan');
      setTimeout(() => location.reload(), 200);
      return;
    } catch (e) {
      $status.textContent = e.message;
    } finally {
      $btnScan.disabled = false;
      $btnScan.textContent = t('toolbar.scan');
    }
  });

  // ---- Restore button ------------------------------------------------------
  $btnRestore.addEventListener('click', () => {
    if (!currentFile || !selectedRestoreChecksum) return;
    openRestoreModal();
  });

  // ---- Filter input --------------------------------------------------------
  let filterTimeout = null;
  $filter.addEventListener('input', () => {
    clearTimeout(filterTimeout);
    filterTimeout = setTimeout(renderFileList, 80);
  });
  const $filterClear = document.querySelector('#sidebar-header .filter-clear');
  if ($filterClear) {
    $filterClear.addEventListener('click', () => {
      $filter.value = '';
      clearTimeout(filterTimeout);
      renderFileList();
      $filter.focus();
    });
  }

  // ---- Hide deleted files --------------------------------------------------
  async function refreshTimelineView() {
    if (tlActiveRangeBtn) {
      const range = tlActiveRangeBtn.dataset.range;
      const now = Date.now();
      let since = range === 'all' ? 0 : now - parseInt(range, 10);
      const until = now;
      try {
        const sinceISO = new Date(since).toISOString();
        const untilISO = new Date(until).toISOString();
        const includeDeleted = !hideDeletedFiles;
        const entries = await apiJson(
          '/api/activity?since=' +
            encodeURIComponent(sinceISO) +
            '&until=' +
            encodeURIComponent(untilISO) +
            '&include_deleted=' +
            includeDeleted
        );
        if (entries.length === 0) {
          tlMode = 'multi';
          tlLanes = [];
          tlViewStart = since || now - 3600000;
          tlViewEnd = until;
        } else {
          let viewStart = since;
          let viewEnd = until;
          if (range === 'all') {
            const first = new Date(entries[0].timestamp).getTime();
            const last = new Date(entries[entries.length - 1].timestamp).getTime();
            const span = Math.max(last - first, MIN_VIEW_SPAN);
            const pad = span * 0.08;
            viewStart = first - pad;
            viewEnd = last + pad;
          }
          setTimelineMultiFile(entries, viewStart, viewEnd);
        }
        updateTimelineLabel();
        updateLaneLabels();
        requestTimelineDraw();
      } catch {
        // Intentionally empty
      }
    } else if (currentFile) {
      tlMode = 'single';
      setTimelineSingleFile(currentFile, shouldShowOnTimeline() ? historyEntries : []);
    }
  }

  $showDeleted.addEventListener('change', () => {
    hideDeletedFiles = !$showDeleted.checked;
    localStorage.setItem(SHOW_DELETED_STORAGE_KEY, $showDeleted.checked ? 'true' : 'false');
    loadFiles();
    refreshTimelineView();
  });

  // ---- Utilities -----------------------------------------------------------
  function escapeHtml(s) {
    const div = document.createElement('div');
    div.textContent = s;
    return div.innerHTML;
  }

  function formatDateTime(d) {
    const y = d.getFullYear();
    const m = String(d.getMonth() + 1).padStart(2, '0');
    const day = String(d.getDate()).padStart(2, '0');
    const h = String(d.getHours()).padStart(2, '0');
    const min = String(d.getMinutes()).padStart(2, '0');
    const s = String(d.getSeconds()).padStart(2, '0');
    return y + '/' + m + '/' + day + ' ' + h + ':' + min + ':' + s;
  }

  function formatSize(bytes) {
    if (bytes < 1024) return bytes + ' B';
    if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
    return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
  }

  async function apiText(path) {
    const res = await api(path);
    return res.text();
  }

  function normalizeLines(text) {
    if (!text) return [];
    const lines = text.split('\n');
    if (lines.length > 0 && lines[lines.length - 1] === '') {
      lines.pop();
    }
    return lines;
  }

  function scrollFileToActive() {
    const node = $fileList.querySelector('.tree-file.active');
    if (node) {
      node.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    }
  }

  function rememberSelectedFile(path) {
    if (!path) {
      localStorage.removeItem(SELECTED_FILE_STORAGE_KEY);
      localStorage.removeItem(SELECTED_FILES_STORAGE_KEY);
      return;
    }
    localStorage.setItem(SELECTED_FILE_STORAGE_KEY, path);
    localStorage.removeItem(SELECTED_FILES_STORAGE_KEY);
  }

  /** Remember multiple selected files (multi-select / directory select). */
  function rememberSelectedFiles(paths) {
    if (!paths || paths.length === 0) {
      localStorage.removeItem(SELECTED_FILE_STORAGE_KEY);
      localStorage.removeItem(SELECTED_FILES_STORAGE_KEY);
      return;
    }
    if (paths.length === 1) {
      localStorage.setItem(SELECTED_FILE_STORAGE_KEY, paths[0]);
      localStorage.removeItem(SELECTED_FILES_STORAGE_KEY);
      return;
    }
    localStorage.setItem(SELECTED_FILES_STORAGE_KEY, JSON.stringify(paths));
    localStorage.setItem(SELECTED_FILE_STORAGE_KEY, paths[0]);
  }

  /** Return array of paths to restore: multi if stored as list, else single. */
  function restoreSelectedFiles() {
    const raw = localStorage.getItem(SELECTED_FILES_STORAGE_KEY);
    if (raw) {
      try {
        const arr = JSON.parse(raw);
        if (Array.isArray(arr) && arr.length > 0) {
          return arr.filter((p) => typeof p === 'string');
        }
      } catch {
        // ignore JSON parse error
      }
    }
    const single = localStorage.getItem(SELECTED_FILE_STORAGE_KEY);
    return single ? [single] : [];
  }

  function treeHasFile(nodes, targetPath, prefix) {
    for (const node of nodes) {
      const fullPath = prefix ? prefix + '/' + node.name : node.name;
      if (node.children) {
        if (treeHasFile(node.children, targetPath, fullPath)) {
          return true;
        }
      } else if (fullPath === targetPath) {
        return true;
      }
    }
    return false;
  }

  function expandDirsForPath(path) {
    if (!path) return;
    const parts = path.split('/').filter(Boolean);
    if (parts.length <= 1) return;
    let current = '';
    for (let i = 0; i < parts.length - 1; i++) {
      current = current ? current + '/' + parts[i] : parts[i];
      collapsedDirs.delete(current);
    }
  }

  function shouldIgnoreKeyboard(e) {
    const target = e.target;
    if (!target) return false;
    const tag = target.tagName;
    if (tag === 'INPUT' || tag === 'TEXTAREA') return true;
    return target.isContentEditable === true;
  }

  function onTimelineKeydown(e) {
    if (shouldIgnoreKeyboard(e)) return;

    if (isMouseOverTimeline && tlLanes.length > 0) {
      const key = e.key.toLowerCase();
      const span = tlViewEnd - tlViewStart;
      if (key === 'a' || key === 'd') {
        e.preventDefault();
        const step = span * 0.075 * (key === 'a' ? -1 : 1);
        tlViewStart += step;
        tlViewEnd += step;
        updateTimelineLabel();
        requestTimelineDraw();
        return;
      }
      if (key === 'w' || key === 's') {
        e.preventDefault();
        const center = (tlViewStart + tlViewEnd) / 2;
        const newSpan = span * (key === 'w' ? 1 / 1.2 : 1.2);
        tlViewStart = center - newSpan / 2;
        tlViewEnd = center + newSpan / 2;
        updateTimelineLabel();
        requestTimelineDraw();
        return;
      }
    }

    if (historyEntries.length === 0) return;
    if (e.key === 'ArrowLeft') {
      e.preventDefault();
      const next = Math.max(0, activeEntryIdx - 1);
      if (next !== activeEntryIdx) {
        selectEntry(next, true);
      }
    } else if (e.key === 'ArrowRight') {
      e.preventDefault();
      const next = Math.min(historyEntries.length - 1, activeEntryIdx + 1);
      if (next !== activeEntryIdx) {
        selectEntry(next, true);
      }
    }
  }

  function onFileListKeydown(e) {
    if (shouldIgnoreKeyboard(e)) return;

    // Z: fit time range to current (filtered) file list on timeline
    if (e.key === 'z' || e.key === 'Z') {
      e.preventDefault();
      fitViewToCurrentFileList();
      return;
    }

    // Shortcuts 1-7: select time range (toggle on second press). Ignored when focus is in filter input (handled by shouldIgnoreKeyboard).
    const digit = e.key >= '1' && e.key <= '7' ? parseInt(e.key, 10) : 0;
    if (digit) {
      const rangeBtns = document.querySelectorAll('.tl-range-btn');
      if (rangeBtns.length >= digit) {
        e.preventDefault();
        onRangeButtonClick(rangeBtns[digit - 1]);
      }
      return;
    }

    if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return;

    // Up/Down: globally affect timeline lane navigation in multi-lane mode; no file list navigation
    if (tlMode === 'multi' && tlLanes.length > 1) {
      e.preventDefault();
      onTimelineLaneSwitch(e.key === 'ArrowDown' ? 1 : -1);
    }
  }

  /** Switch to an adjacent lane in multi-file timeline, selecting the closest node in time */
  function onTimelineLaneSwitch(direction) {
    const visibleIndices = getVisibleLaneIndices();
    if (visibleIndices.length === 0) return;

    let currentLaneIdx = -1;
    if (tlActiveNode) {
      currentLaneIdx = tlActiveNode.laneIdx;
    } else {
      for (let i = 0; i < tlLanes.length; i++) {
        if (tlLanes[i].file === currentFile) {
          currentLaneIdx = i;
          break;
        }
      }
    }

    const currentVi = currentLaneIdx === -1 ? -1 : visibleIndices.indexOf(currentLaneIdx);
    let targetVi;
    if (currentVi === -1) {
      targetVi = direction > 0 ? 0 : visibleIndices.length - 1;
    } else {
      targetVi = currentVi + direction;
    }

    if (targetVi < 0 || targetVi >= visibleIndices.length) return;
    const targetLaneIdx = visibleIndices[targetVi];

    const targetLane = tlLanes[targetLaneIdx];
    if (targetLane.entries.length === 0) return;

    // Find the entry in target lane closest in time to the current active node
    let targetEntryIdx = 0;
    if (tlActiveNode) {
      const refTs = new Date(tlActiveNode.entry.timestamp).getTime();
      let bestDist = Infinity;
      for (let i = 0; i < targetLane.entries.length; i++) {
        const ts = new Date(targetLane.entries[i].timestamp).getTime();
        const dist = Math.abs(ts - refTs);
        if (dist < bestDist) {
          bestDist = dist;
          targetEntryIdx = i;
        }
      }
    } else {
      // Default to latest entry
      targetEntryIdx = targetLane.entries.length - 1;
    }

    const targetEntry = targetLane.entries[targetEntryIdx];
    const syntheticNode = {
      laneIdx: targetLaneIdx,
      entryIdx: targetEntryIdx,
      entry: targetEntry,
    };

    onNodeClick(syntheticNode);
    updateLaneLabels();

    // Flash tooltip to give visual feedback
    requestAnimationFrame(() => {
      requestAnimationFrame(() => flashTooltipAtActiveNode());
    });
  }

  // ---- Init ----------------------------------------------------------------
  function ensureFirstVisitLayoutDefaults() {
    const SIDEBAR_KEY = 'ftm-sidebar-width';
    const w = window.innerWidth;
    const width18 = Math.round(w * 0.18);
    const width21 = Math.round(w * 0.21);
    if (!localStorage.getItem(SIDEBAR_KEY)) localStorage.setItem(SIDEBAR_KEY, String(width21));
    if (!localStorage.getItem(TL_HEIGHT_STORAGE_KEY))
      localStorage.setItem(TL_HEIGHT_STORAGE_KEY, String(width18));
    if (!localStorage.getItem(TL_LANES_WIDTH_STORAGE_KEY))
      localStorage.setItem(TL_LANES_WIDTH_STORAGE_KEY, String(width21));
  }

  async function init() {
    // Initialize i18n
    var $langSelect = document.getElementById('lang-select');
    if ($langSelect) {
      $langSelect.value = I18N.getLang();
      $langSelect.addEventListener('change', function () {
        I18N.setLang($langSelect.value);
      });
    }
    I18N.applyI18n();

    const savedShowDeleted = localStorage.getItem(SHOW_DELETED_STORAGE_KEY);
    if (savedShowDeleted !== null) {
      $showDeleted.checked = savedShowDeleted === 'true' || savedShowDeleted === '1';
    }
    hideDeletedFiles = !$showDeleted.checked;
    ensureFirstVisitLayoutDefaults();
    initDragResize({
      handle: $resizeHandle,
      target: $sidebar,
      storageKey: 'ftm-sidebar-width',
      axis: 'x',
      sizeProp: 'width',
      minSize: 120,
      getMaxSize: function () {
        return window.innerWidth * 0.5;
      },
      bodyClass: 'resizing',
    });
    initTimeline();
    initTreeDepthButtons();

    try {
      const health = await apiJson('/api/health');
      if (health.watch_dir) {
        $status.textContent = health.watch_dir;
        if ($toolbarStats) $toolbarStats.setAttribute('aria-hidden', 'false');
        await loadFiles();
        await loadStats();
        const storedPaths = restoreSelectedFiles().filter((p) => treeHasFile(fileTree, p, ''));
        if (storedPaths.length > 1) {
          storedPaths.forEach((p) => expandDirsForPath(p));
          currentFile = storedPaths[0];
          selectedFiles = new Set(storedPaths);
          renderFileList();
          await selectMultipleFiles(storedPaths);
        } else if (storedPaths.length === 1) {
          expandDirsForPath(storedPaths[0]);
          await selectFile(storedPaths[0]);
        } else {
          requestTimelineDraw();
        }
        const savedRange = localStorage.getItem(TIMELINE_RANGE_STORAGE_KEY);
        if (savedRange) {
          const rangeBtn = document.querySelector('.tl-range-btn[data-range="' + savedRange + '"]');
          if (rangeBtn) await onRangeButtonClick(rangeBtn);
        }
      } else {
        if ($toolbarStats) $toolbarStats.setAttribute('aria-hidden', 'true');
        $status.textContent = t('status.noCheckout');
        $diffViewer.innerHTML = '<div class="empty-state">' + t('status.checkoutHint') + '</div>';
        requestTimelineDraw();
      }
    } catch {
      if ($toolbarStats) $toolbarStats.setAttribute('aria-hidden', 'true');
      $status.textContent = t('status.serverUnreachable');
      $diffViewer.innerHTML =
        '<div class="empty-state">' + escapeHtml(t('status.noConnect')) + '</div>';
      requestTimelineDraw();
    }

    document.addEventListener('keydown', onTimelineKeydown);
    document.addEventListener('keydown', onFileListKeydown);

    // Help modal
    function isHelpOpen() {
      return $helpOverlay.getAttribute('aria-hidden') === 'false';
    }
    function openHelp() {
      $helpOverlay.setAttribute('aria-hidden', 'false');
      $helpOverlay.classList.add('help-open');
      if ($helpTitle) $helpTitle.focus();
    }
    function closeHelp() {
      $helpOverlay.setAttribute('aria-hidden', 'true');
      $helpOverlay.classList.remove('help-open');
      if ($btnHelp) $btnHelp.focus();
    }
    $btnHelp.addEventListener('click', () => openHelp());
    $helpOverlay.addEventListener('click', (e) => {
      if (e.target === $helpOverlay) closeHelp();
    });
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && isHelpOpen()) {
        e.preventDefault();
        closeHelp();
      }
    });
  }

  // ---- Restore modal -------------------------------------------------------
  const RESTORE_ROW_HEIGHT = 20;
  const RESTORE_OVERSCAN = 20;
  let restoreRows = []; // unified row data for both panels
  let restoreSyncing = false; // guard against recursive scroll sync
  let restoreRafLeft = null;
  let restoreRafRight = null;
  let restoreLeftTable = null;
  let restoreRightTable = null;

  // Synced scrolling (set up once)
  $restorePanelLeft.addEventListener('scroll', function () {
    if (restoreSyncing || restoreRows.length === 0) return;
    restoreSyncing = true;
    $restorePanelRight.scrollTop = $restorePanelLeft.scrollTop;
    restoreSyncing = false;
    scheduleRestoreRender('right');
    scheduleRestoreRender('left');
  });

  $restorePanelRight.addEventListener('scroll', function () {
    if (restoreSyncing || restoreRows.length === 0) return;
    restoreSyncing = true;
    $restorePanelLeft.scrollTop = $restorePanelRight.scrollTop;
    restoreSyncing = false;
    scheduleRestoreRender('left');
    scheduleRestoreRender('right');
  });

  function isRestoreOpen() {
    return $restoreOverlay.getAttribute('aria-hidden') === 'false';
  }

  function getLatestChecksum() {
    for (let i = historyEntries.length - 1; i >= 0; i--) {
      if (historyEntries[i].checksum) return historyEntries[i].checksum;
    }
    return null;
  }

  async function openRestoreModal() {
    if (!currentFile || !selectedRestoreChecksum) return;

    const latestChecksum = getLatestChecksum();
    const isDeleted =
      historyEntries.length > 0 && historyEntries[historyEntries.length - 1].op === 'delete';

    // Populate info panel
    $restoreFilepath.textContent = currentFile;
    $restoreInfoFilename.textContent = currentFile;
    $restoreInfoFrom.textContent = isDeleted
      ? '\u2014'
      : latestChecksum
        ? latestChecksum.slice(0, 12)
        : '\u2014';
    $restoreInfoTo.textContent = selectedRestoreChecksum.slice(0, 12);
    $restoreInfoStats.innerHTML = '';

    // Show loading state
    $restorePanelLeft.innerHTML =
      '<div class="restore-loading">' + escapeHtml(t('restore.loading')) + '</div>';
    $restorePanelRight.innerHTML =
      '<div class="restore-loading">' + escapeHtml(t('restore.loading')) + '</div>';

    // Show modal
    $restoreOverlay.setAttribute('aria-hidden', 'false');
    $restoreConfirm.disabled = false;
    $restoreCancel.disabled = false;
    $restoreConfirm.focus();

    // Fetch diff: from=current, to=restore target. When file is deleted, do not pass from so
    // server diffs empty vs target and we see the full restore.
    try {
      let url = '/api/diff?to=' + encodeURIComponent(selectedRestoreChecksum);
      if (latestChecksum && !isDeleted) {
        url += '&from=' + encodeURIComponent(latestChecksum);
      }
      const diff = await apiJson(url);
      renderRestorePreview(diff);
    } catch (e) {
      $restorePanelLeft.innerHTML =
        '<div class="restore-loading">' + escapeHtml(e.message) + '</div>';
      $restorePanelRight.innerHTML =
        '<div class="restore-loading">' + escapeHtml(e.message) + '</div>';
    }
  }

  function closeRestoreModal() {
    $restoreOverlay.setAttribute('aria-hidden', 'true');
    restoreRows = [];
    $restorePanelLeft.innerHTML = '';
    $restorePanelRight.innerHTML = '';
    restoreLeftTable = null;
    restoreRightTable = null;
    $btnRestore.focus();
  }

  function buildRestoreRows(diff) {
    var rows = [];

    for (var hi = 0; hi < diff.hunks.length; hi++) {
      var hunk = diff.hunks[hi];

      // Separator before hunk
      if (hi > 0 || hunk.old_start > 1 || hunk.new_start > 1) {
        var skipped;
        if (hi === 0) {
          skipped = Math.max(hunk.old_start - 1, hunk.new_start - 1);
        } else {
          var prev = diff.hunks[hi - 1];
          var prevEndOld = hunkEndLine(prev, 'old');
          var prevEndNew = hunkEndLine(prev, 'new');
          skipped = Math.max(hunk.old_start - prevEndOld, hunk.new_start - prevEndNew);
        }
        if (skipped > 0) {
          rows.push({
            type: 'separator',
            text: t('diff.unchangedLines', { n: skipped }),
          });
        }
      }

      // Lines
      var oldLine = hunk.old_start;
      var newLine = hunk.new_start;
      for (var li = 0; li < hunk.lines.length; li++) {
        var line = hunk.lines[li];
        if (line.tag === 'equal') {
          rows.push({
            type: 'equal',
            leftNum: oldLine++,
            leftContent: line.content,
            rightNum: newLine++,
            rightContent: line.content,
          });
        } else if (line.tag === 'delete') {
          rows.push({
            type: 'delete',
            leftNum: oldLine++,
            leftContent: line.content,
            rightNum: null,
            rightContent: null,
          });
        } else if (line.tag === 'insert') {
          rows.push({
            type: 'insert',
            leftNum: null,
            leftContent: null,
            rightNum: newLine++,
            rightContent: line.content,
          });
        }
      }
    }

    // Trailing separator
    if (diff.hunks.length > 0) {
      var lastHunk = diff.hunks[diff.hunks.length - 1];
      var lastEndOld = hunkEndLine(lastHunk, 'old');
      var lastEndNew = hunkEndLine(lastHunk, 'new');
      var remaining = Math.max(diff.old_total - lastEndOld + 1, diff.new_total - lastEndNew + 1);
      if (remaining > 0) {
        rows.push({
          type: 'separator',
          text: t('diff.unchangedLines', { n: remaining }),
        });
      }
    }

    return rows;
  }

  function renderRestorePreview(diff) {
    restoreRows = buildRestoreRows(diff);

    // Compute stats
    var added = 0;
    var removed = 0;
    for (var i = 0; i < restoreRows.length; i++) {
      if (restoreRows[i].type === 'insert') added++;
      if (restoreRows[i].type === 'delete') removed++;
    }

    // Update stats display
    $restoreInfoStats.innerHTML = '';
    if (added === 0 && removed === 0) {
      $restoreInfoStats.innerHTML =
        '<div class="restore-info-value" style="color:var(--fg-dim)">' +
        escapeHtml(t('restore.noChanges')) +
        '</div>';
    } else {
      var statsDiv = document.createElement('div');
      statsDiv.className = 'restore-info-stat';
      if (added > 0) {
        var addSpan = document.createElement('span');
        addSpan.className = 'restore-stat-add';
        addSpan.textContent = t('restore.linesAdded', { n: added });
        statsDiv.appendChild(addSpan);
      }
      if (removed > 0) {
        var delSpan = document.createElement('span');
        delSpan.className = 'restore-stat-del';
        delSpan.textContent = t('restore.linesRemoved', { n: removed });
        statsDiv.appendChild(delSpan);
      }
      $restoreInfoStats.appendChild(statsDiv);
    }

    if (restoreRows.length === 0) {
      $restorePanelLeft.innerHTML =
        '<div class="restore-no-changes">' + escapeHtml(t('restore.noChanges')) + '</div>';
      $restorePanelRight.innerHTML =
        '<div class="restore-no-changes">' + escapeHtml(t('restore.noChanges')) + '</div>';
      return;
    }

    // Initialize virtual scroll for both panels
    initRestoreVirtualScroll($restorePanelLeft, 'left');
    initRestoreVirtualScroll($restorePanelRight, 'right');

    renderRestoreVisibleRows('left');
    renderRestoreVisibleRows('right');
  }

  function initRestoreVirtualScroll(container, side) {
    container.innerHTML = '';

    var spacer = document.createElement('div');
    spacer.style.height = restoreRows.length * RESTORE_ROW_HEIGHT + 'px';
    spacer.style.position = 'relative';

    var table = document.createElement('table');
    table.className = 'restore-diff-table';

    var colgroup = document.createElement('colgroup');
    var colGutter = document.createElement('col');
    colGutter.style.width = '50px';
    var colCode = document.createElement('col');
    colgroup.appendChild(colGutter);
    colgroup.appendChild(colCode);
    table.appendChild(colgroup);

    var tbody = document.createElement('tbody');
    table.appendChild(tbody);

    spacer.appendChild(table);
    container.appendChild(spacer);

    if (side === 'left') {
      restoreLeftTable = table;
    } else {
      restoreRightTable = table;
    }
  }

  function scheduleRestoreRender(side) {
    if (side === 'left') {
      if (restoreRafLeft) return;
      restoreRafLeft = requestAnimationFrame(function () {
        restoreRafLeft = null;
        renderRestoreVisibleRows('left');
      });
    } else {
      if (restoreRafRight) return;
      restoreRafRight = requestAnimationFrame(function () {
        restoreRafRight = null;
        renderRestoreVisibleRows('right');
      });
    }
  }

  function renderRestoreVisibleRows(side) {
    var container = side === 'left' ? $restorePanelLeft : $restorePanelRight;
    var table = side === 'left' ? restoreLeftTable : restoreRightTable;
    if (!container || !table || restoreRows.length === 0) return;

    var scrollTop = container.scrollTop;
    var viewHeight = container.clientHeight;

    var startIdx = Math.floor(scrollTop / RESTORE_ROW_HEIGHT) - RESTORE_OVERSCAN;
    var endIdx = Math.ceil((scrollTop + viewHeight) / RESTORE_ROW_HEIGHT) + RESTORE_OVERSCAN;
    startIdx = Math.max(0, startIdx);
    endIdx = Math.min(restoreRows.length, endIdx);

    table.style.top = startIdx * RESTORE_ROW_HEIGHT + 'px';

    var tbody = table.querySelector('tbody');
    var frag = document.createDocumentFragment();

    for (var i = startIdx; i < endIdx; i++) {
      var row = restoreRows[i];
      var tr = document.createElement('tr');
      tr.style.height = RESTORE_ROW_HEIGHT + 'px';

      if (row.type === 'separator') {
        tr.className = 'rdiff-separator';
        var td = document.createElement('td');
        td.colSpan = 2;
        td.textContent = row.text;
        tr.appendChild(td);
      } else {
        var isLeft = side === 'left';
        var lineNum = isLeft ? row.leftNum : row.rightNum;
        var content = isLeft ? row.leftContent : row.rightContent;
        var hasContent = content != null;

        // Determine row class
        if (row.type === 'equal') {
          tr.className = 'rdiff-line-equal';
        } else if (row.type === 'delete') {
          tr.className = isLeft ? 'rdiff-line-delete' : 'rdiff-line-placeholder';
        } else if (row.type === 'insert') {
          tr.className = isLeft ? 'rdiff-line-placeholder' : 'rdiff-line-insert';
        }

        var gutterTd = document.createElement('td');
        gutterTd.className = 'rdiff-gutter';
        gutterTd.textContent = lineNum != null ? lineNum : '';

        var codeTd = document.createElement('td');
        codeTd.className = 'rdiff-code';
        codeTd.textContent = hasContent ? content : '';

        tr.appendChild(gutterTd);
        tr.appendChild(codeTd);
      }

      frag.appendChild(tr);
    }

    tbody.innerHTML = '';
    tbody.appendChild(frag);
  }

  const REFRESH_AFTER_RESTORE_TIMEOUT_MS = 1000;
  const REFRESH_AFTER_RESTORE_POLL_MS = 300;

  async function refreshAfterRestore() {
    if (!currentFile) return;
    const deadline = Date.now() + REFRESH_AFTER_RESTORE_TIMEOUT_MS;
    const prevLen = historyEntries.length;
    while (Date.now() < deadline) {
      try {
        const entries = await apiJson('/api/history?file=' + encodeURIComponent(currentFile));
        if (entries.length > prevLen) break;
      } catch {
        // ignore poll errors
      }
      await new Promise(function (r) {
        setTimeout(r, REFRESH_AFTER_RESTORE_POLL_MS);
      });
    }
    try {
      const hist = await apiJson('/api/history?file=' + encodeURIComponent(currentFile));
      historyEntries = hist;
      if (historyEntries.length === 0) {
        requestTimelineDraw();
        updateTimelineLabel();
        updateLaneLabels();
        return;
      }
      // Update lane data in place: keep time scale and file list unchanged
      const li = tlLanes.findIndex(function (l) {
        return l.file === currentFile;
      });
      if (li >= 0) {
        tlLanes[li].entries = hist;
      } else if (tlLanes.length === 1 && tlLanes[0].file === currentFile) {
        tlLanes[0].entries = hist;
      } else {
        tlLanes = hist.length > 0 ? [{ file: currentFile, entries: hist }] : [];
      }
      selectEntryDiff(historyEntries.length - 1);
      ensureActiveNodeInView();
      updateTimelineLabel();
      updateLaneLabels();
      requestTimelineDraw();
    } catch {
      // keep current state on error
    }
  }

  async function executeRestore() {
    if (!currentFile || !selectedRestoreChecksum) return;
    $restoreConfirm.disabled = true;
    $restoreCancel.disabled = true;
    try {
      await apiPost('/api/restore', {
        file: currentFile,
        checksum: selectedRestoreChecksum,
      });
      $status.textContent = t('status.restoreRequested', { file: currentFile });
      closeRestoreModal();
      refreshAfterRestore();
    } catch (e) {
      $status.textContent = e.message;
      $restoreConfirm.disabled = false;
      $restoreCancel.disabled = false;
    }
  }

  // Restore modal event listeners
  $restoreCancel.addEventListener('click', function () {
    closeRestoreModal();
  });

  $restoreConfirm.addEventListener('click', function () {
    executeRestore();
  });

  $restoreOverlay.addEventListener('click', function (e) {
    if (e.target === $restoreOverlay) closeRestoreModal();
  });

  document.addEventListener('keydown', function (e) {
    if (e.key === 'Escape' && isRestoreOpen()) {
      e.preventDefault();
      closeRestoreModal();
    }
  });

  init();
})();
