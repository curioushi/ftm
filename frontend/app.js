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
  const TL_HEIGHT_STORAGE_KEY = 'ftm-timeline-height';
  const TL_LANES_WIDTH_STORAGE_KEY = 'ftm-tl-lanes-width';

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

  // Canvas drawing constants
  const LANE_HEIGHT = 24;
  const RULER_HEIGHT = 22;
  const NODE_RADIUS = 5;
  const NODE_HIT_RADIUS = 8;
  const MIN_VIEW_SPAN = 60 * 1000; // 1 minute minimum zoom

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

  function opColor(op) {
    if (op === 'create') return COLORS.green;
    if (op === 'modify') return COLORS.blue;
    if (op === 'delete') return COLORS.red;
    return COLORS.fgDim;
  }

  // ---- DOM refs ------------------------------------------------------------
  const $filter = document.getElementById('filter');
  const $hideDeleted = document.getElementById('hide-deleted');
  const $fileList = document.getElementById('file-list');
  const $diffViewer = document.getElementById('diff-viewer');
  const $diffTitle = document.getElementById('diff-title');
  const $diffMeta = document.getElementById('diff-meta');
  const $btnRestore = document.getElementById('btn-restore');
  const $timelineLabel = document.getElementById('timeline-label');
  const $btnScan = document.getElementById('btn-scan');
  const $status = document.getElementById('status');
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

  // ---- File list -----------------------------------------------------------

  async function loadFiles() {
    try {
      const includeDeleted = !hideDeletedFiles;
      const q = includeDeleted ? '?include_deleted=true' : '';
      fileTree = await apiJson('/api/files' + q);
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
      $fileList.innerHTML = '<div class="empty-state">No files</div>';
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
    for (const node of nodes) {
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
          if (collapsedDirs.has(fullPath)) {
            collapsedDirs.delete(fullPath);
          } else {
            collapsedDirs.add(fullPath);
          }
          renderFileList();
        });

        // Dir name click: select all files under this directory
        nameSpan.addEventListener('click', (e) => {
          e.stopPropagation();
          // Ensure expanded so user can see selected files
          collapsedDirs.delete(fullPath);
          const childFiles = collectFilesUnder(node.children, fullPath);
          if (childFiles.length === 0) return;
          selectedFiles = new Set(childFiles);
          currentFile = childFiles[0];
          rememberSelectedFile(currentFile);
          renderFileList();
          selectMultipleFiles(childFiles);
        });

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

  // ---- Tree depth buttons ---------------------------------------------------
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

  function initTreeDepthButtons() {
    const buttons = document.querySelectorAll('.tree-depth-btn');
    buttons.forEach((btn) => {
      btn.addEventListener('click', () => {
        const action = btn.dataset.action;
        const dirs = collectAllDirPaths(fileTree, '', 0, []);

        if (action === 'expand-all') {
          collapsedDirs.clear();
        } else if (action === 'collapse-all') {
          collapsedDirs.clear();
          for (const d of dirs) {
            collapsedDirs.add(d.path);
          }
        } else if (action.startsWith('depth-')) {
          const maxDepth = parseInt(action.replace('depth-', ''), 10);
          collapsedDirs.clear();
          for (const d of dirs) {
            if (d.depth >= maxDepth) {
              collapsedDirs.add(d.path);
            }
          }
        }
        renderFileList();
      });
    });
  }

  // ---- Sidebar resize -------------------------------------------------------
  function initSidebarResize() {
    const STORAGE_KEY = 'ftm-sidebar-width';
    const MIN_WIDTH = 120;
    const MAX_WIDTH_RATIO = 0.5;

    // Restore saved width
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved) {
      const w = parseInt(saved, 10);
      if (w >= MIN_WIDTH) {
        $sidebar.style.width = w + 'px';
      }
    }

    let startX = 0;
    let startWidth = 0;

    function onMouseDown(e) {
      e.preventDefault();
      startX = e.clientX;
      startWidth = $sidebar.getBoundingClientRect().width;
      $resizeHandle.classList.add('dragging');
      document.body.classList.add('resizing');
      document.addEventListener('mousemove', onMouseMove);
      document.addEventListener('mouseup', onMouseUp);
    }

    function onMouseMove(e) {
      const maxW = window.innerWidth * MAX_WIDTH_RATIO;
      let newWidth = startWidth + (e.clientX - startX);
      newWidth = Math.max(MIN_WIDTH, Math.min(maxW, newWidth));
      $sidebar.style.width = newWidth + 'px';
    }

    function onMouseUp() {
      $resizeHandle.classList.remove('dragging');
      document.body.classList.remove('resizing');
      document.removeEventListener('mousemove', onMouseMove);
      document.removeEventListener('mouseup', onMouseUp);
      // Persist
      const w = Math.round($sidebar.getBoundingClientRect().width);
      localStorage.setItem(STORAGE_KEY, String(w));
    }

    $resizeHandle.addEventListener('mousedown', onMouseDown);
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
    $diffViewer.innerHTML = '<div class="loading">Loading history...</div>';

    // Clear active range button when selecting a specific file
    clearActiveRangeBtn();

    try {
      historyEntries = await apiJson('/api/history?file=' + encodeURIComponent(path));
      tlMode = 'single';
      const showOnTimeline =
        !hideDeletedFiles ||
        historyEntries.length === 0 ||
        historyEntries[historyEntries.length - 1].op !== 'delete';
      setTimelineSingleFile(path, showOnTimeline ? historyEntries : []);
      // Auto-select latest entry
      if (historyEntries.length > 0) {
        selectEntry(historyEntries.length - 1);
      } else {
        $diffViewer.innerHTML = '<div class="empty-state">No history</div>';
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
    clearActiveRangeBtn();
    $diffTitle.textContent = files.length + ' files selected';
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
      $status.textContent = 'Failed to load history: ' + e.message;
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

    // Sort files alphabetically
    const sortedFiles = Array.from(byFile.keys()).sort();
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
      $timelineLabel.textContent = 'No activity';
      return;
    }
    const s = new Date(tlViewStart);
    const e = new Date(tlViewEnd);
    $timelineLabel.textContent = formatDateTime(s) + ' \u2014 ' + formatDateTime(e);
  }

  function updateLaneLabels() {
    $timelineLanes.innerHTML = '';

    // Add a spacer matching the ruler height so labels align with canvas lanes
    const spacer = document.createElement('div');
    spacer.className = 'tl-lane-spacer';
    spacer.style.height = RULER_HEIGHT + 'px';
    spacer.style.flexShrink = '0';
    $timelineLanes.appendChild(spacer);

    for (let i = 0; i < tlLanes.length; i++) {
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
      label.addEventListener('dblclick', () => {
        if (tlMode === 'multi') {
          selectFile(file);
        }
      });
      $timelineLanes.appendChild(label);
    }
    // Sync scroll
    $timelineLanes.scrollTop = tlScrollY;
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

    // Choose tick interval
    const tickIntervals = [
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

    // Clip to lanes area
    ctx.beginPath();
    ctx.rect(0, lanesAreaTop, w, lanesAreaHeight);
    ctx.clip();

    for (let li = 0; li < tlLanes.length; li++) {
      const lane = tlLanes[li];
      const laneY = lanesAreaTop + li * LANE_HEIGHT - tlScrollY;

      // Skip if not visible
      if (laneY + LANE_HEIGHT < lanesAreaTop || laneY > h) continue;

      // Lane background - subtle alternating
      if (li % 2 === 1) {
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

      // Draw nodes
      for (const ve of visibleEntries) {
        const { x, ei, entry } = ve;
        // Skip nodes that are way off screen
        if (x < -NODE_RADIUS * 2 || x > w + NODE_RADIUS * 2) continue;

        const color = opColor(entry.op);
        const isActive =
          tlActiveNode && tlActiveNode.laneIdx === li && tlActiveNode.entryIdx === ei;
        const isHovered =
          tlHoveredNode && tlHoveredNode.laneIdx === li && tlHoveredNode.entryIdx === ei;

        // Draw node
        ctx.beginPath();
        ctx.arc(x, centerY, isHovered ? NODE_RADIUS + 1.5 : NODE_RADIUS, 0, Math.PI * 2);

        if (isActive) {
          ctx.fillStyle = color;
          ctx.fill();
          // Active ring
          ctx.strokeStyle = COLORS.fgBright;
          ctx.lineWidth = 2;
          ctx.stroke();
          // White center highlight
          ctx.beginPath();
          ctx.arc(x, centerY, 2, 0, Math.PI * 2);
          ctx.fillStyle = COLORS.fgBright;
          ctx.fill();
        } else {
          ctx.fillStyle = COLORS.bg;
          ctx.fill();
          ctx.strokeStyle = color;
          ctx.lineWidth = 2;
          ctx.stroke();
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
    let closest = null;
    let closestDist = NODE_HIT_RADIUS + 1;

    for (let li = 0; li < tlLanes.length; li++) {
      const lane = tlLanes[li];
      const laneY = lanesAreaTop + li * LANE_HEIGHT - tlScrollY;
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

      // Pan vertically
      const maxScrollY = Math.max(
        0,
        tlLanes.length * LANE_HEIGHT - (parseFloat($canvas.style.height) - RULER_HEIGHT)
      );
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

    if (!wasDrag) {
      // Click - select node
      const { x, y } = getCanvasCoords(e);
      const node = hitTestNode(x, y);
      if (node) {
        onNodeClick(node);
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

      let newSpan = (tlViewEnd - tlViewStart) * factor;
      newSpan = Math.max(MIN_VIEW_SPAN, newSpan);

      // Find max span from all entries
      let allMin = Infinity;
      let allMax = -Infinity;
      for (const lane of tlLanes) {
        for (const ent of lane.entries) {
          const ts = new Date(ent.timestamp).getTime();
          if (ts < allMin) allMin = ts;
          if (ts > allMax) allMax = ts;
        }
      }
      if (allMax > allMin) {
        const maxSpan = (allMax - allMin) * 3;
        newSpan = Math.min(newSpan, Math.max(maxSpan, MIN_VIEW_SPAN * 100));
      }

      tlViewStart = mouseTime - leftRatio * newSpan;
      tlViewEnd = mouseTime + (1 - leftRatio) * newSpan;
    } else {
      // Default wheel: vertical scroll lanes
      const maxScrollY = Math.max(
        0,
        tlLanes.length * LANE_HEIGHT - (parseFloat($canvas.style.height) - RULER_HEIGHT)
      );
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
      rememberSelectedFile(lane.file);
      renderFileList();
      updateLaneLabels();
      $diffTitle.textContent = lane.file;

      // Load the full history for this file to enable diff viewing
      apiJson('/api/history?file=' + encodeURIComponent(lane.file)).then((entries) => {
        historyEntries = entries;
        // Find the matching entry index
        const clickedEntry = node.entry;
        let matchIdx = entries.length - 1;
        for (let i = 0; i < entries.length; i++) {
          if (
            entries[i].timestamp === clickedEntry.timestamp &&
            entries[i].checksum === clickedEntry.checksum
          ) {
            matchIdx = i;
            break;
          }
        }
        selectEntryDiff(matchIdx);
      });
    } else {
      // Single-file mode or same file - just select the entry
      const clickedEntry = node.entry;
      let matchIdx = -1;
      for (let i = 0; i < historyEntries.length; i++) {
        if (
          historyEntries[i].timestamp === clickedEntry.timestamp &&
          historyEntries[i].checksum === clickedEntry.checksum
        ) {
          matchIdx = i;
          break;
        }
      }
      if (matchIdx >= 0) {
        selectEntryDiff(matchIdx);
      }
    }

    requestTimelineDraw();
  }

  function showTooltipAtNode(node) {
    const entry = node.entry;
    let text = entry.op + ' \u2022 ' + formatDateTime(new Date(entry.timestamp));
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
    const laneY = RULER_HEIGHT + node.laneIdx * LANE_HEIGHT - tlScrollY;
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
      $diffViewer.innerHTML = '<div class="empty-state">File was deleted in this version</div>';
      $diffMeta.textContent = entry.op + ' \u2022 ' + formatDateTime(new Date(entry.timestamp));
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

    $diffMeta.textContent = entry.op + ' \u2022 ' + formatDateTime(new Date(entry.timestamp));
    if (entry.size != null) {
      $diffMeta.textContent += ' \u2022 ' + formatSize(entry.size);
    }

    $diffViewer.innerHTML = '<div class="loading">Computing diff...</div>';

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

  /** Find the lane index and lane-local entry index for the current file's history entry */
  function matchActiveNodeForEntry(entry) {
    if (!entry || tlLanes.length === 0) return null;
    if (tlMode === 'single') {
      // In single mode, lane 0 maps directly to historyEntries index
      for (let ei = 0; ei < tlLanes[0].entries.length; ei++) {
        const le = tlLanes[0].entries[ei];
        if (le.timestamp === entry.timestamp && le.checksum === entry.checksum) {
          return { laneIdx: 0, entryIdx: ei, entry: le };
        }
      }
      return null;
    }
    // Multi-file mode: find the lane for currentFile
    for (let li = 0; li < tlLanes.length; li++) {
      if (tlLanes[li].file !== currentFile) continue;
      const lane = tlLanes[li];
      for (let ei = 0; ei < lane.entries.length; ei++) {
        const le = lane.entries[ei];
        if (le.timestamp === entry.timestamp && le.checksum === entry.checksum) {
          return { laneIdx: li, entryIdx: ei, entry: le };
        }
      }
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
        const showOnTimeline =
          !hideDeletedFiles ||
          historyEntries.length === 0 ||
          historyEntries[historyEntries.length - 1].op !== 'delete';
        setTimelineSingleFile(currentFile, showOnTimeline ? historyEntries : []);
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
    } catch (e) {
      $status.textContent = 'Activity load failed: ' + e.message;
    }
  }

  // ---- Timeline vertical resize ---------------------------------------------
  function initTimelineResize() {
    const MIN_HEIGHT = 60;
    const MAX_HEIGHT_RATIO = 0.5;

    // Restore saved height
    const saved = localStorage.getItem(TL_HEIGHT_STORAGE_KEY);
    if (saved) {
      const h = parseInt(saved, 10);
      if (h >= MIN_HEIGHT) {
        $timelineBar.style.height = h + 'px';
      }
    }

    let startY = 0;
    let startHeight = 0;

    function onMouseDown(e) {
      e.preventDefault();
      startY = e.clientY;
      startHeight = $timelineBar.getBoundingClientRect().height;
      $tlResizeHandle.classList.add('dragging');
      document.body.classList.add('tl-resizing');
      document.addEventListener('mousemove', onMouseMove);
      document.addEventListener('mouseup', onMouseUp);
    }

    function onMouseMove(e) {
      const maxH = window.innerHeight * MAX_HEIGHT_RATIO;
      // Dragging up increases height (resize handle is above timeline)
      let newHeight = startHeight - (e.clientY - startY);
      newHeight = Math.max(MIN_HEIGHT, Math.min(maxH, newHeight));
      $timelineBar.style.height = newHeight + 'px';
      requestTimelineDraw();
    }

    function onMouseUp() {
      $tlResizeHandle.classList.remove('dragging');
      document.body.classList.remove('tl-resizing');
      document.removeEventListener('mousemove', onMouseMove);
      document.removeEventListener('mouseup', onMouseUp);
      // Persist
      const h = Math.round($timelineBar.getBoundingClientRect().height);
      localStorage.setItem(TL_HEIGHT_STORAGE_KEY, String(h));
      requestTimelineDraw();
    }

    $tlResizeHandle.addEventListener('mousedown', onMouseDown);
  }

  // ---- Timeline lanes resize -------------------------------------------------
  function initTimelineLanesResize() {
    const MIN_WIDTH = 60;
    const MAX_WIDTH_RATIO = 0.4;

    // Restore saved width
    const saved = localStorage.getItem(TL_LANES_WIDTH_STORAGE_KEY);
    if (saved) {
      const w = parseInt(saved, 10);
      if (w >= MIN_WIDTH) {
        $timelineLanes.style.width = w + 'px';
      }
    }

    let startX = 0;
    let startWidth = 0;

    function onMouseDown(e) {
      e.preventDefault();
      startX = e.clientX;
      startWidth = $timelineLanes.getBoundingClientRect().width;
      $tlLanesResize.classList.add('dragging');
      document.body.classList.add('tl-lanes-resizing');
      document.addEventListener('mousemove', onMouseMove);
      document.addEventListener('mouseup', onMouseUp);
    }

    function onMouseMove(e) {
      const maxW = $timelineBody.getBoundingClientRect().width * MAX_WIDTH_RATIO;
      let newWidth = startWidth + (e.clientX - startX);
      newWidth = Math.max(MIN_WIDTH, Math.min(maxW, newWidth));
      $timelineLanes.style.width = newWidth + 'px';
      requestTimelineDraw();
    }

    function onMouseUp() {
      $tlLanesResize.classList.remove('dragging');
      document.body.classList.remove('tl-lanes-resizing');
      document.removeEventListener('mousemove', onMouseMove);
      document.removeEventListener('mouseup', onMouseUp);
      const w = Math.round($timelineLanes.getBoundingClientRect().width);
      localStorage.setItem(TL_LANES_WIDTH_STORAGE_KEY, String(w));
      requestTimelineDraw();
    }

    $tlLanesResize.addEventListener('mousedown', onMouseDown);
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
    initTimelineResize();
    initTimelineLanesResize();
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
      $diffViewer.innerHTML = '<div class="empty-state">No changes</div>';
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
          const prevEndOld = prevHunkEndOld(prev);
          const prevEndNew = prevHunkEndNew(prev);
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
            text: '\u00B7\u00B7\u00B7 ' + skipped + ' unchanged lines \u00B7\u00B7\u00B7',
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
    const lastEndOld = prevHunkEndOld(lastHunk);
    const lastEndNew = prevHunkEndNew(lastHunk);
    if (lastEndOld <= diff.old_total || lastEndNew <= diff.new_total) {
      const remaining = Math.max(diff.old_total - lastEndOld + 1, diff.new_total - lastEndNew + 1);
      if (remaining > 0) {
        diffRows.push({
          type: 'separator',
          text: '\u00B7\u00B7\u00B7 ' + remaining + ' unchanged lines \u00B7\u00B7\u00B7',
          oldFrom: lastEndOld,
          oldTo: diff.old_total,
          newFrom: lastEndNew,
          newTo: diff.new_total,
        });
      }
    }

    initVirtualScroll();
  }

  function prevHunkEndOld(hunk) {
    let line = hunk.old_start;
    for (const l of hunk.lines) {
      if (l.tag === 'equal' || l.tag === 'delete') line++;
    }
    return line;
  }

  function prevHunkEndNew(hunk) {
    let line = hunk.new_start;
    for (const l of hunk.lines) {
      if (l.tag === 'equal' || l.tag === 'insert') line++;
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
      row.text = '(failed to load snapshot)';
      renderVisibleRows();
    }
  }

  // ---- Scan button ---------------------------------------------------------
  $btnScan.addEventListener('click', async () => {
    $btnScan.disabled = true;
    $btnScan.textContent = 'Scanning...';
    try {
      const result = await apiPost('/api/scan');
      $status.textContent =
        'Scan: +' + result.created + ' ~' + result.modified + ' -' + result.deleted;
      $btnScan.disabled = false;
      $btnScan.textContent = 'Scan';
      setTimeout(() => location.reload(), 200);
      return;
    } catch (e) {
      $status.textContent = e.message;
    } finally {
      $btnScan.disabled = false;
      $btnScan.textContent = 'Scan';
    }
  });

  // ---- Restore button ------------------------------------------------------
  $btnRestore.addEventListener('click', async () => {
    if (!currentFile || !selectedRestoreChecksum) return;
    const shortChecksum = selectedRestoreChecksum.slice(0, 12);
    const ok = window.confirm('Restore to version ' + shortChecksum + '?');
    if (!ok) return;
    $btnRestore.disabled = true;
    try {
      await apiPost('/api/restore', {
        file: currentFile,
        checksum: selectedRestoreChecksum,
      });
      $status.textContent = 'Restore requested for ' + currentFile;
      $btnRestore.disabled = false;
      setTimeout(() => location.reload(), 200);
      return;
    } catch (e) {
      $status.textContent = e.message;
    } finally {
      $btnRestore.disabled = false;
    }
  });

  // ---- Filter input --------------------------------------------------------
  let filterTimeout = null;
  $filter.addEventListener('input', () => {
    clearTimeout(filterTimeout);
    filterTimeout = setTimeout(renderFileList, 80);
  });

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
      const showOnTimeline =
        !hideDeletedFiles ||
        historyEntries.length === 0 ||
        historyEntries[historyEntries.length - 1].op !== 'delete';
      setTimelineSingleFile(currentFile, showOnTimeline ? historyEntries : []);
    }
  }

  $hideDeleted.addEventListener('change', () => {
    hideDeletedFiles = $hideDeleted.checked;
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
    return d.toLocaleString(undefined, {
      year: 'numeric',
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
    });
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
      return;
    }
    localStorage.setItem(SELECTED_FILE_STORAGE_KEY, path);
  }

  function restoreSelectedFile() {
    return localStorage.getItem(SELECTED_FILE_STORAGE_KEY);
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
      if (key === 'a') {
        e.preventDefault();
        const step = span * 0.075;
        tlViewStart -= step;
        tlViewEnd -= step;
        updateTimelineLabel();
        requestTimelineDraw();
        return;
      }
      if (key === 'd') {
        e.preventDefault();
        const step = span * 0.075;
        tlViewStart += step;
        tlViewEnd += step;
        updateTimelineLabel();
        requestTimelineDraw();
        return;
      }
      if (key === 'w') {
        e.preventDefault();
        const center = (tlViewStart + tlViewEnd) / 2;
        let newSpan = span / 1.2;
        newSpan = Math.max(MIN_VIEW_SPAN, newSpan);
        tlViewStart = center - newSpan / 2;
        tlViewEnd = center + newSpan / 2;
        updateTimelineLabel();
        requestTimelineDraw();
        return;
      }
      if (key === 's') {
        e.preventDefault();
        const center = (tlViewStart + tlViewEnd) / 2;
        let newSpan = span * 1.2;
        let allMin = Infinity;
        let allMax = -Infinity;
        for (const lane of tlLanes) {
          for (const ent of lane.entries) {
            const ts = new Date(ent.timestamp).getTime();
            if (ts < allMin) allMin = ts;
            if (ts > allMax) allMax = ts;
          }
        }
        if (allMax > allMin) {
          const maxSpan = (allMax - allMin) * 3;
          newSpan = Math.min(newSpan, Math.max(maxSpan, MIN_VIEW_SPAN * 100));
        }
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
    if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return;

    // When mouse is over timeline in multi-lane mode, switch lanes instead
    if (isMouseOverTimeline && tlMode === 'multi' && tlLanes.length > 1) {
      e.preventDefault();
      onTimelineLaneSwitch(e.key === 'ArrowDown' ? 1 : -1);
      return;
    }

    if (visibleFilePaths.length === 0) return;

    e.preventDefault();
    let idx = visibleFilePaths.indexOf(currentFile);
    if (idx === -1) {
      idx = e.key === 'ArrowDown' ? -1 : visibleFilePaths.length;
    }
    const next =
      e.key === 'ArrowDown' ? Math.min(visibleFilePaths.length - 1, idx + 1) : Math.max(0, idx - 1);
    if (visibleFilePaths[next]) {
      selectFile(visibleFilePaths[next]);
    }
  }

  /** Switch to an adjacent lane in multi-file timeline, selecting the closest node in time */
  function onTimelineLaneSwitch(direction) {
    let currentLaneIdx = -1;
    if (tlActiveNode) {
      currentLaneIdx = tlActiveNode.laneIdx;
    } else {
      // Try to find lane by currentFile
      for (let i = 0; i < tlLanes.length; i++) {
        if (tlLanes[i].file === currentFile) {
          currentLaneIdx = i;
          break;
        }
      }
    }

    // Compute target lane index
    let targetLaneIdx;
    if (currentLaneIdx === -1) {
      targetLaneIdx = direction > 0 ? 0 : tlLanes.length - 1;
    } else {
      targetLaneIdx = currentLaneIdx + direction;
    }

    // Clamp
    if (targetLaneIdx < 0 || targetLaneIdx >= tlLanes.length) return;

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
  async function init() {
    hideDeletedFiles = $hideDeleted.checked;
    initSidebarResize();
    initTimeline();
    initTreeDepthButtons();

    try {
      const health = await apiJson('/api/health');
      if (health.watch_dir) {
        $status.textContent = health.watch_dir;
        await loadFiles();
        const storedFile = restoreSelectedFile();
        if (storedFile && treeHasFile(fileTree, storedFile, '')) {
          expandDirsForPath(storedFile);
          await selectFile(storedFile);
        } else {
          // No stored file - show empty timeline
          requestTimelineDraw();
        }
      } else {
        $status.textContent = 'No directory checked out';
        $diffViewer.innerHTML =
          '<div class="empty-state">Run <code>ftm checkout &lt;dir&gt;</code> first</div>';
        requestTimelineDraw();
      }
    } catch {
      $status.textContent = 'Server unreachable';
      $diffViewer.innerHTML = '<div class="empty-state">Cannot connect to FTM server</div>';
      requestTimelineDraw();
    }

    document.addEventListener('keydown', onTimelineKeydown);
    document.addEventListener('keydown', onFileListKeydown);
  }

  init();
})();
