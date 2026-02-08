// FTM Web UI - Application Logic
// ===========================================================================

(function () {
  "use strict";

  // ---- State ---------------------------------------------------------------
  let allFiles = [];          // [{path, count}, ...]
  let currentFile = null;     // selected file path
  let historyEntries = [];    // history for current file
  let activeEntryIdx = -1;    // selected timeline node index
  let diffRows = [];          // flat array of rendered diff row data

  // ---- DOM refs ------------------------------------------------------------
  const $filter = document.getElementById("filter");
  const $fileList = document.getElementById("file-list");
  const $diffViewer = document.getElementById("diff-viewer");
  const $diffTitle = document.getElementById("diff-title");
  const $diffMeta = document.getElementById("diff-meta");
  const $timeline = document.getElementById("timeline");
  const $timelineLabel = document.getElementById("timeline-label");
  const $btnScan = document.getElementById("btn-scan");
  const $status = document.getElementById("status");

  // ---- API helpers ---------------------------------------------------------
  const API = "";

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
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: body ? JSON.stringify(body) : undefined,
    });
    if (!res.ok) {
      const data = await res.json().catch(() => ({ message: res.statusText }));
      throw new Error(data.message || res.statusText);
    }
    return res.json();
  }

  // ---- File list -----------------------------------------------------------

  // The /api/files endpoint returns a tree of FileTreeNode:
  //   { name, count?, children? }
  // Flatten it into [{path, count}] for the UI.
  function flattenTree(nodes, prefix) {
    const result = [];
    for (const node of nodes) {
      const fullPath = prefix ? prefix + "/" + node.name : node.name;
      if (node.children) {
        result.push(...flattenTree(node.children, fullPath));
      } else {
        result.push({ path: fullPath, count: node.count || 0 });
      }
    }
    return result;
  }

  async function loadFiles() {
    try {
      const tree = await apiJson("/api/files");
      allFiles = flattenTree(tree, "");
      renderFileList();
    } catch (e) {
      $status.textContent = e.message;
    }
  }

  function renderFileList() {
    const query = $filter.value.toLowerCase();
    const filtered = query
      ? allFiles.filter((f) => f.path.toLowerCase().includes(query))
      : allFiles;

    $fileList.innerHTML = "";

    if (filtered.length === 0) {
      $fileList.innerHTML = '<div class="empty-state">No files</div>';
      return;
    }

    const frag = document.createDocumentFragment();
    for (const f of filtered) {
      const el = document.createElement("div");
      el.className = "file-item" + (f.path === currentFile ? " active" : "");
      el.innerHTML =
        '<span class="name">' + escapeHtml(f.path) + "</span>" +
        '<span class="count">' + f.count + "</span>";
      el.addEventListener("click", () => selectFile(f.path));
      frag.appendChild(el);
    }
    $fileList.appendChild(frag);
  }

  async function selectFile(path) {
    currentFile = path;
    renderFileList();
    $diffTitle.textContent = path;
    $diffMeta.textContent = "";
    $diffViewer.innerHTML = '<div class="loading">Loading history...</div>';

    try {
      historyEntries = await apiJson(
        "/api/history?file=" + encodeURIComponent(path)
      );
      renderTimeline();
      // Auto-select latest entry
      if (historyEntries.length > 0) {
        selectEntry(historyEntries.length - 1);
      } else {
        $diffViewer.innerHTML = '<div class="empty-state">No history</div>';
      }
    } catch (e) {
      $diffViewer.innerHTML =
        '<div class="empty-state">' + escapeHtml(e.message) + "</div>";
    }
  }

  // ---- Timeline ------------------------------------------------------------
  function renderTimeline() {
    $timeline.innerHTML = "";
    $timelineLabel.textContent = "";

    if (historyEntries.length === 0) {
      $timelineLabel.textContent = "No history";
      return;
    }

    const first = new Date(historyEntries[0].timestamp);
    const last = new Date(historyEntries[historyEntries.length - 1].timestamp);
    $timelineLabel.textContent = formatDate(first) + " \u2014 " + formatDate(last);

    const frag = document.createDocumentFragment();
    for (let i = 0; i < historyEntries.length; i++) {
      const entry = historyEntries[i];

      // Track segment before node (except first)
      if (i > 0) {
        const track = document.createElement("div");
        track.className = "tl-track";
        frag.appendChild(track);
      }

      const node = document.createElement("div");
      node.className =
        "tl-node op-" + entry.op + (i === activeEntryIdx ? " active" : "");
      node.dataset.idx = i;

      const tooltip = document.createElement("div");
      tooltip.className = "tl-tooltip";
      tooltip.textContent =
        entry.op + " \u2022 " + formatDateTime(new Date(entry.timestamp));
      if (entry.size != null) {
        tooltip.textContent += " \u2022 " + formatSize(entry.size);
      }
      node.appendChild(tooltip);

      node.addEventListener("click", () => selectEntry(i));
      frag.appendChild(node);
    }
    $timeline.appendChild(frag);
  }

  async function selectEntry(idx) {
    activeEntryIdx = idx;
    const entry = historyEntries[idx];

    // Update timeline active state
    const nodes = $timeline.querySelectorAll(".tl-node");
    nodes.forEach((n, i) => {
      n.classList.toggle("active", i === idx);
    });

    // Build diff query
    const toChecksum = entry.checksum;

    if (!toChecksum) {
      // Delete event - show message
      $diffViewer.innerHTML =
        '<div class="empty-state">File was deleted in this version</div>';
      $diffMeta.textContent = entry.op + " \u2022 " + formatDateTime(new Date(entry.timestamp));
      return;
    }

    // Find previous checksum
    let fromChecksum = null;
    for (let i = idx - 1; i >= 0; i--) {
      if (historyEntries[i].checksum) {
        fromChecksum = historyEntries[i].checksum;
        break;
      }
    }

    $diffMeta.textContent = entry.op + " \u2022 " + formatDateTime(new Date(entry.timestamp));
    if (entry.size != null) {
      $diffMeta.textContent += " \u2022 " + formatSize(entry.size);
    }

    $diffViewer.innerHTML = '<div class="loading">Computing diff...</div>';

    try {
      let url =
        "/api/diff?file=" +
        encodeURIComponent(currentFile) +
        "&to=" +
        encodeURIComponent(toChecksum);
      if (fromChecksum) {
        url += "&from=" + encodeURIComponent(fromChecksum);
      }
      const diff = await apiJson(url);
      renderDiff(diff);
    } catch (e) {
      $diffViewer.innerHTML =
        '<div class="empty-state">' + escapeHtml(e.message) + "</div>";
    }
  }

  // ---- Diff renderer -------------------------------------------------------
  // Uses a virtual-scroll approach: we build a flat list of row descriptors and
  // only materialize DOM rows inside the visible viewport.

  const ROW_HEIGHT = 20; // must match CSS line-height
  const OVERSCAN = 20;   // extra rows above/below viewport

  function renderDiff(diff) {
    diffRows = [];

    if (diff.hunks.length === 0) {
      $diffViewer.innerHTML = '<div class="empty-state">No changes</div>';
      return;
    }

    // Build flat row list with separators between hunks
    for (let hi = 0; hi < diff.hunks.length; hi++) {
      const hunk = diff.hunks[hi];

      // Separator before hunk (skip for the very first if it starts at line 1)
      if (hi > 0 || hunk.old_start > 1 || hunk.new_start > 1) {
        let skippedOld = 0;
        let skippedNew = 0;
        if (hi === 0) {
          skippedOld = hunk.old_start - 1;
          skippedNew = hunk.new_start - 1;
        } else {
          const prev = diff.hunks[hi - 1];
          const prevEndOld = prevHunkEndOld(prev);
          const prevEndNew = prevHunkEndNew(prev);
          skippedOld = hunk.old_start - prevEndOld;
          skippedNew = hunk.new_start - prevEndNew;
        }
        const skipped = Math.max(skippedOld, skippedNew);
        if (skipped > 0) {
          diffRows.push({
            type: "separator",
            text: "\u00B7\u00B7\u00B7 " + skipped + " unchanged lines \u00B7\u00B7\u00B7",
            oldStart: hunk.old_start,
            newStart: hunk.new_start,
            fromChecksum: null,
            toChecksum: null,
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
        if (line.tag === "equal") {
          row.oldNum = oldLine++;
          row.newNum = newLine++;
        } else if (line.tag === "delete") {
          row.oldNum = oldLine++;
        } else if (line.tag === "insert") {
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
      const remaining = Math.max(
        diff.old_total - lastEndOld + 1,
        diff.new_total - lastEndNew + 1
      );
      if (remaining > 0) {
        diffRows.push({
          type: "separator",
          text: "\u00B7\u00B7\u00B7 " + remaining + " unchanged lines \u00B7\u00B7\u00B7",
        });
      }
    }

    initVirtualScroll();
  }

  function prevHunkEndOld(hunk) {
    let line = hunk.old_start;
    for (const l of hunk.lines) {
      if (l.tag === "equal" || l.tag === "delete") line++;
    }
    return line;
  }

  function prevHunkEndNew(hunk) {
    let line = hunk.new_start;
    for (const l of hunk.lines) {
      if (l.tag === "equal" || l.tag === "insert") line++;
    }
    return line;
  }

  // ---- Virtual scroll ------------------------------------------------------
  let vsContainer = null; // the scrollable wrapper
  let vsSpacer = null;    // tall empty div for scroll height
  let vsContent = null;   // positioned div holding visible rows
  let vsRafId = null;

  function initVirtualScroll() {
    $diffViewer.innerHTML = "";

    vsContainer = $diffViewer;
    vsSpacer = document.createElement("div");
    vsSpacer.style.height = diffRows.length * ROW_HEIGHT + "px";
    vsSpacer.style.position = "relative";

    vsContent = document.createElement("table");
    vsContent.className = "diff-table";
    vsContent.style.position = "absolute";
    vsContent.style.left = "0";
    vsContent.style.right = "0";
    vsContent.style.willChange = "transform";

    vsSpacer.appendChild(vsContent);
    vsContainer.appendChild(vsSpacer);

    vsContainer.addEventListener("scroll", onVsScroll, { passive: true });
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

    vsContent.style.top = startIdx * ROW_HEIGHT + "px";

    // Build rows
    const frag = document.createDocumentFragment();
    for (let i = startIdx; i < endIdx; i++) {
      const row = diffRows[i];
      const tr = document.createElement("tr");
      tr.style.height = ROW_HEIGHT + "px";

      if (row.type === "separator") {
        tr.className = "diff-separator";
        const td = document.createElement("td");
        td.colSpan = 3;
        td.textContent = row.text;
        td.addEventListener("click", () => expandSeparator(i));
        tr.appendChild(td);
      } else {
        tr.className = "diff-line-" + row.type;

        const gutterOld = document.createElement("td");
        gutterOld.className = "diff-gutter diff-gutter-old";
        gutterOld.textContent = row.oldNum != null ? row.oldNum : "";

        const gutterNew = document.createElement("td");
        gutterNew.className = "diff-gutter diff-gutter-new";
        gutterNew.textContent = row.newNum != null ? row.newNum : "";

        const code = document.createElement("td");
        code.className = "diff-code";
        code.textContent = row.content;

        tr.appendChild(gutterOld);
        tr.appendChild(gutterNew);
        tr.appendChild(code);
      }

      frag.appendChild(tr);
    }

    vsContent.innerHTML = "";
    vsContent.appendChild(frag);
  }

  async function expandSeparator(rowIdx) {
    // For now, separators are informational only.
    // A full implementation would fetch the unchanged lines from the snapshot
    // and splice them in. This is a placeholder.
    const row = diffRows[rowIdx];
    if (row && row.type === "separator") {
      row.text = "(expanded - full content can be fetched via snapshot API)";
      renderVisibleRows();
    }
  }

  // ---- Scan button ---------------------------------------------------------
  $btnScan.addEventListener("click", async () => {
    $btnScan.disabled = true;
    $btnScan.textContent = "Scanning...";
    try {
      const result = await apiPost("/api/scan");
      $status.textContent =
        "Scan: +" +
        result.created +
        " ~" +
        result.modified +
        " -" +
        result.deleted;
      await loadFiles();
      // Refresh current file if selected
      if (currentFile) {
        await selectFile(currentFile);
      }
    } catch (e) {
      $status.textContent = e.message;
    } finally {
      $btnScan.disabled = false;
      $btnScan.textContent = "Scan";
    }
  });

  // ---- Filter input --------------------------------------------------------
  let filterTimeout = null;
  $filter.addEventListener("input", () => {
    clearTimeout(filterTimeout);
    filterTimeout = setTimeout(renderFileList, 80);
  });

  // ---- Utilities -----------------------------------------------------------
  function escapeHtml(s) {
    const div = document.createElement("div");
    div.textContent = s;
    return div.innerHTML;
  }

  function formatDate(d) {
    return d.toLocaleDateString(undefined, {
      year: "numeric",
      month: "short",
      day: "numeric",
    });
  }

  function formatDateTime(d) {
    return d.toLocaleString(undefined, {
      year: "numeric",
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
  }

  function formatSize(bytes) {
    if (bytes < 1024) return bytes + " B";
    if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + " KB";
    return (bytes / (1024 * 1024)).toFixed(1) + " MB";
  }

  // ---- Init ----------------------------------------------------------------
  async function init() {
    try {
      const health = await apiJson("/api/health");
      if (health.watch_dir) {
        $status.textContent = health.watch_dir;
        await loadFiles();
      } else {
        $status.textContent = "No directory checked out";
        $diffViewer.innerHTML =
          '<div class="empty-state">Run <code>ftm checkout &lt;dir&gt;</code> first</div>';
      }
    } catch (e) {
      $status.textContent = "Server unreachable";
      $diffViewer.innerHTML =
        '<div class="empty-state">Cannot connect to FTM server</div>';
    }
  }

  init();
})();
