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

  // ---- DOM refs ------------------------------------------------------------
  const $filter = document.getElementById('filter');
  const $hideDeleted = document.getElementById('hide-deleted');
  const $fileList = document.getElementById('file-list');
  const $diffViewer = document.getElementById('diff-viewer');
  const $diffTitle = document.getElementById('diff-title');
  const $diffMeta = document.getElementById('diff-meta');
  const $btnRestore = document.getElementById('btn-restore');
  const $timeline = document.getElementById('timeline');
  const $timelineLabel = document.getElementById('timeline-label');
  const $btnScan = document.getElementById('btn-scan');
  const $status = document.getElementById('status');
  const $sidebar = document.getElementById('sidebar');
  const $resizeHandle = document.getElementById('resize-handle');

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

        dirRow.appendChild(arrow);
        dirRow.appendChild(nameSpan);
        dirRow.addEventListener('click', () => {
          if (collapsedDirs.has(fullPath)) {
            collapsedDirs.delete(fullPath);
          } else {
            collapsedDirs.add(fullPath);
          }
          renderFileList();
        });

        parent.appendChild(dirRow);

        const childContainer = document.createElement('div');
        childContainer.className = 'tree-children' + (isCollapsed ? ' collapsed' : '');
        renderTreeNodes(childContainer, node.children, fullPath, depth + 1, forceExpand);
        parent.appendChild(childContainer);
      } else {
        // File node
        const fileRow = document.createElement('div');
        fileRow.className = 'tree-file' + (fullPath === currentFile ? ' active' : '');
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
        fileRow.addEventListener('click', () => selectFile(fullPath));

        parent.appendChild(fileRow);
      }
    }
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
    selectedRestoreChecksum = null;
    updateRestoreButton();
    renderFileList();
    $diffTitle.textContent = path;
    $diffMeta.textContent = '';
    $diffViewer.innerHTML = '<div class="loading">Loading history...</div>';

    try {
      historyEntries = await apiJson('/api/history?file=' + encodeURIComponent(path));
      renderTimeline();
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

  // ---- Timeline ------------------------------------------------------------
  function updateRestoreButton() {
    if (!currentFile || !selectedRestoreChecksum) {
      $btnRestore.classList.remove('is-visible');
      return;
    }
    $btnRestore.classList.add('is-visible');
  }

  function renderTimeline() {
    $timeline.innerHTML = '';
    $timelineLabel.textContent = '';

    if (historyEntries.length === 0) {
      $timelineLabel.textContent = 'No history';
      return;
    }

    const first = new Date(historyEntries[0].timestamp);
    const last = new Date(historyEntries[historyEntries.length - 1].timestamp);
    $timelineLabel.textContent = formatDate(first) + ' \u2014 ' + formatDate(last);

    const frag = document.createDocumentFragment();
    for (let i = 0; i < historyEntries.length; i++) {
      const entry = historyEntries[i];

      // Track segment before node (except first)
      if (i > 0) {
        const track = document.createElement('div');
        track.className = 'tl-track';
        frag.appendChild(track);
      }

      const node = document.createElement('div');
      node.className = 'tl-node op-' + entry.op + (i === activeEntryIdx ? ' active' : '');
      node.dataset.idx = i;

      const tooltip = document.createElement('div');
      tooltip.className = 'tl-tooltip';
      tooltip.textContent = entry.op + ' \u2022 ' + formatDateTime(new Date(entry.timestamp));
      if (entry.size != null) {
        tooltip.textContent += ' \u2022 ' + formatSize(entry.size);
      }
      node.appendChild(tooltip);

      node.addEventListener('click', () => selectEntry(i));
      frag.appendChild(node);
    }
    $timeline.appendChild(frag);
  }

  async function selectEntry(idx) {
    activeEntryIdx = idx;
    const entry = historyEntries[idx];
    selectedRestoreChecksum = entry && entry.checksum ? entry.checksum : null;
    updateRestoreButton();

    // Update timeline active state
    const nodes = $timeline.querySelectorAll('.tl-node');
    nodes.forEach((n, i) => {
      n.classList.toggle('active', i === idx);
    });
    scrollTimelineToActive();

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
      await loadFiles();
      // Refresh current file if selected
      if (currentFile) {
        await selectFile(currentFile);
      }
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
      await selectFile(currentFile);
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
  $hideDeleted.addEventListener('change', () => {
    hideDeletedFiles = $hideDeleted.checked;
    loadFiles();
  });

  // ---- Utilities -----------------------------------------------------------
  function escapeHtml(s) {
    const div = document.createElement('div');
    div.textContent = s;
    return div.innerHTML;
  }

  function formatDate(d) {
    return d.toLocaleDateString(undefined, {
      year: 'numeric',
      month: 'short',
      day: 'numeric',
    });
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

  function scrollTimelineToActive() {
    if (activeEntryIdx < 0) return;
    const node = $timeline.querySelector('.tl-node.active');
    if (node) {
      node.scrollIntoView({ block: 'nearest', inline: 'center' });
    }
  }

  function scrollFileToActive() {
    const node = $fileList.querySelector('.tree-file.active');
    if (node) {
      node.scrollIntoView({ block: 'nearest', inline: 'nearest' });
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
    if (historyEntries.length === 0) return;
    if (e.key === 'ArrowLeft') {
      e.preventDefault();
      const next = Math.max(0, activeEntryIdx - 1);
      if (next !== activeEntryIdx) {
        selectEntry(next);
      }
    } else if (e.key === 'ArrowRight') {
      e.preventDefault();
      const next = Math.min(historyEntries.length - 1, activeEntryIdx + 1);
      if (next !== activeEntryIdx) {
        selectEntry(next);
      }
    }
  }

  function onFileListKeydown(e) {
    if (shouldIgnoreKeyboard(e)) return;
    if (visibleFilePaths.length === 0) return;
    if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return;

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

  // ---- Init ----------------------------------------------------------------
  async function init() {
    hideDeletedFiles = $hideDeleted.checked;
    initSidebarResize();

    try {
      const health = await apiJson('/api/health');
      if (health.watch_dir) {
        $status.textContent = health.watch_dir;
        await loadFiles();
      } else {
        $status.textContent = 'No directory checked out';
        $diffViewer.innerHTML =
          '<div class="empty-state">Run <code>ftm checkout &lt;dir&gt;</code> first</div>';
      }
    } catch {
      $status.textContent = 'Server unreachable';
      $diffViewer.innerHTML = '<div class="empty-state">Cannot connect to FTM server</div>';
    }

    document.addEventListener('keydown', onTimelineKeydown);
    document.addEventListener('keydown', onFileListKeydown);
  }

  init();
})();
