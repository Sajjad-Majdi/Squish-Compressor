const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;
const { getCurrentWebview } = window.__TAURI__.webview;

const open = (options = {}) => invoke("plugin:dialog|open", { options });
const revealItemInDir = (path) => invoke("plugin:opener|reveal_item_in_dir", {
  paths: Array.isArray(path) ? path : [path],
});

const VIDEO = new Set(["mp4", "mov", "m4v", "mkv", "webm", "avi", "wmv", "flv", "mpg", "mpeg", "ts", "m2ts", "3gp"]);
const IMAGE = new Set(["jpg", "jpeg", "png", "webp"]);

const $ = (s) => document.querySelector(s);
const listEl = $("#list");
const dropzone = $("#dropzone");
const actionbar = $("#actionbar");
const cancelBtn = $("#cancel-run");
const clearBtn = $("#clear");
const summaryEl = $("#summary");
const sessionBox = $("#session");
const sessionNum = $("#session-num");

const escapeHtml = (value) => String(value).replace(/[&<>"']/g, (char) => ({
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&#39;",
})[char]);

const files = new Map();
let mode = localStorage.getItem("squish.mode") || "balanced";
let outDir = null;
let running = false;
let sessionSaved = 0;
let autoStartTimer = null;

const fmtSize = (b) => {
  if (b == null) return "";
  if (b < 1024) return `${b} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = b / 1024, i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return `${v >= 100 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
};

function toast(msg) {
  const t = document.createElement("div");
  t.className = "toast";
  t.textContent = msg;
  $("#toast-layer").appendChild(t);
  setTimeout(() => { t.classList.add("out"); setTimeout(() => t.remove(), 350); }, 3400);
}

function iconSvg(kind) {
  return `<svg><use href="#i-${kind === "video" ? "film" : "image"}"/></svg>`;
}

function makeRow(f) {
  const li = document.createElement("li");
  li.className = `row ${f.kind}`;
  li.id = f.id;
  li.style.setProperty("--i", files.size % 8);
  li.innerHTML = `
    <div class="tile ${f.kind}">${iconSvg(f.kind)}</div>
    <div class="meta">
      <div class="name" title="${escapeHtml(f.path)}">${escapeHtml(f.name)}</div>
      <div class="info">${fmtSize(f.size)} · ${f.kind === "video" ? "Video" : "Image"}</div>
      <div class="bar"><i></i></div>
    </div>
    <div class="right"></div>
  `;
  const right = li.querySelector(".right");
  const removeBtn = document.createElement("button");
  removeBtn.className = "rowact show remove";
  removeBtn.title = "Remove";
  removeBtn.innerHTML = `<svg><use href="#i-x"/></svg>`;
  removeBtn.addEventListener("click", () => {
    if (files.get(f.id)?.status === "working") return;
    files.delete(f.id);
    li.remove();
    refreshChrome();
  });
  right.appendChild(removeBtn);
  return li;
}

function setStatus(f, html, extraClass) {
  const li = document.getElementById(f.id);
  if (!li) return;
  const right = li.querySelector(".right");
  right.querySelectorAll(".chip,.err-msg,.rowact.reveal").forEach((n) => n.remove());
  if (extraClass) li.classList.add(extraClass);
  right.insertAdjacentHTML("beforeend", html);
}

function setProgress(f, pct) {
  const li = document.getElementById(f.id);
  if (!li) return;
  li.classList.add("working");
  li.querySelector(".bar i").style.transform = `scaleX(${Math.max(0, Math.min(100, pct)) / 100})`;
}

function finishProgress(f) {
  const li = document.getElementById(f.id);
  if (!li) return;
  li.classList.remove("working");
  li.querySelector(".bar i").style.transform = "scaleX(0)";
}

function addRevealButton(f) {
  if (!f.outPath) return;
  const li = document.getElementById(f.id);
  if (!li) return;
  const btn = document.createElement("button");
  btn.className = "rowact show reveal";
  btn.title = "Show in folder";
  btn.innerHTML = `<svg><use href="#i-reveal"/></svg>`;
  btn.addEventListener("click", async () => {
    try { await revealItemInDir(f.outPath); } catch { toast("Could not open the folder."); }
  });
  li.querySelector(".right").prepend(btn);
}

async function addPaths(paths) {
  let added = false;
  for (const p of paths) {
    const name = p.split(/[\\/]/).pop();
    const ext = name.includes(".") ? name.split(".").pop().toLowerCase() : "";
    const id = crypto.randomUUID();
    if (!VIDEO.has(ext) && !IMAGE.has(ext)) {
      const f = { id, path: p, name, ext, kind: IMAGE.has(ext) ? "image" : "video", size: null, status: "error", message: `${ext ? "." + ext.toUpperCase() : "This file type"} isn't supported` };
      files.set(id, f);
      listEl.appendChild(makeRow(f));
      setStatus(f, `<span class="err-msg" title="${escapeHtml(f.message)}">${escapeHtml(f.message)}</span>`, "error");
      continue;
    }
    let size = null;
    try { size = await invoke("file_size", { path: p }); } catch {}
    const f = { id, path: p, name, ext, kind: IMAGE.has(ext) ? "image" : "video", size, status: "pending", outPath: null };
    files.set(id, f);
    listEl.appendChild(makeRow(f));
    added = true;
  }
  refreshChrome();
  if (added) {
    listEl.scrollTop = listEl.scrollHeight;
    scheduleStart();
  }
}

function counts() {
  let pending = 0, working = 0, finished = 0;
  for (const f of files.values()) {
    if (f.status === "pending") pending++;
    else if (f.status === "working") working++;
    else finished++;
  }
  return { pending, working, finished };
}

function refreshChrome() {
  const { pending } = counts();
  const hasItems = files.size > 0;

  actionbar.classList.toggle("hidden", !hasItems);
  dropzone.classList.toggle("compact", hasItems);

  clearBtn.disabled = running;
  document.querySelectorAll(".seg button").forEach((button) => { button.disabled = running; });
  $("#dest-btn").disabled = running;
  $("#dest-reset").disabled = running;

  const ready = [...files.values()].filter((f) => f.status === "pending");
  const totalReady = ready.reduce((a, f) => a + (f.size || 0), 0);
  const totalSaved = [...files.values()].reduce((a, f) => a + (f.saved || 0), 0);

  if (running) {
    const w = [...files.values()].find((f) => f.status === "working");
    summaryEl.innerHTML = w
      ? `Squeezing <b>${w.name}</b>`
      : `Preparing…`;
  } else if (pending === 0 && totalSaved === 0 && hasItems && counts().finished === files.size) {
    const optimal = [...files.values()].filter((f) => f.status === "optimal").length;
    const failed = [...files.values()].filter((f) => f.status === "error").length;
    summaryEl.innerHTML = optimal > 0
      ? `Nothing to save — <b>${optimal}</b> file${optimal === 1 ? " was" : "s were"} already optimal`
      : failed > 0
        ? `<b>${failed}</b> file${failed === 1 ? "" : "s"} could not be compressed`
        : "";
  } else if (totalSaved > 0 && pending === 0) {
    summaryEl.innerHTML = `All done — saved <span class="ok">${fmtSize(totalSaved)}</span> across <b>${[...files.values()].filter((f) => f.status === "done").length}</b> file${[...files.values()].filter((f) => f.status === "done").length === 1 ? "" : "s"}`;
  } else if (ready.length > 0) {
    summaryEl.innerHTML = `<b>${ready.length}</b> file${ready.length === 1 ? "" : "s"} ready · ${fmtSize(totalReady)}`;
  } else {
    summaryEl.textContent = "";
  }

  if (sessionSaved > 0) {
    sessionBox.classList.remove("hidden");
    sessionNum.textContent = fmtSize(sessionSaved);
  }
}

function scheduleStart() {
  if (autoStartTimer !== null) clearTimeout(autoStartTimer);
  autoStartTimer = setTimeout(() => {
    autoStartTimer = null;
    startRun();
  }, 0);
}

async function startRun() {
  const queue = [...files.values()].filter((f) => f.status === "pending");
  if (queue.length === 0 || running) return;
  running = true;
  refreshChrome();
  cancelBtn.classList.remove("hidden");
  try {
    await invoke("compress_files", {
      files: queue.map((f) => ({ id: f.id, path: f.path })),
      mode,
      outDir,
    });
  } catch (e) {
    toast(`Something went wrong: ${e}`);
    running = false;
    cancelBtn.classList.add("hidden");
    refreshChrome();
  }
}

listen("job-event", (e) => {
  const ev = e.payload;
  const f = files.get(ev.id);
  if (!f) return;
  if (ev.status === "working") {
    f.status = "working";
    setProgress(f, ev.progress);
    refreshChrome();
  } else if (ev.status === "done") {
    f.status = "done";
    f.saved = ev.saved;
    f.outPath = ev.outPath;
    f.finalSize = ev.finalSize;
    sessionSaved += ev.saved;
    finishProgress(f);
    const pct = Math.round((ev.saved / ev.originalSize) * 100);
    setStatus(
      f,
      `<span class="chip saved"><svg fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round"><use href="#i-check"/></svg>&minus;${pct}% · ${fmtSize(ev.finalSize)}</span>`,
      "done-row"
    );
    addRevealButton(f);
    refreshChrome();
  } else if (ev.status === "optimal") {
    f.status = "optimal";
    finishProgress(f);
    setStatus(f, `<span class="chip neutral">Already optimal</span>`);
    refreshChrome();
  } else if (ev.status === "error") {
    f.status = "error";
    f.message = ev.message || "Compression failed";
    finishProgress(f);
    setStatus(f, `<span class="err-msg" title="${escapeHtml(f.message)}">${escapeHtml(f.message)}</span>`, "error");
    refreshChrome();
  } else if (ev.status === "cancelled") {
    if (f.status !== "done") {
      f.status = "cancelled";
      finishProgress(f);
      setStatus(f, `<span class="chip pct">Cancelled</span>`);
    }
    refreshChrome();
  }
});

listen("queue-done", () => {
  running = false;
  cancelBtn.classList.add("hidden");
  for (const f of files.values()) {
    if (f.status === "working") {
      f.status = "cancelled";
      finishProgress(f);
      setStatus(f, `<span class="chip pct">Cancelled</span>`);
    }
  }
  refreshChrome();
  if (counts().pending > 0) scheduleStart();
});

dropzone.addEventListener("click", (e) => {
  if (e.target.closest("#browse")) return;
  browse();
});
$("#browse").addEventListener("click", browse);
dropzone.addEventListener("keydown", (e) => { if (e.key === "Enter") browse(); });

async function browse() {
  const picked = await open({
    multiple: true,
    filters: [{ name: "Videos & images", extensions: [...VIDEO, ...IMAGE] }],
  });
  if (picked) addPaths(picked);
}

getCurrentWebview().onDragDropEvent((ev) => {
  if (ev.payload.type === "over") dropzone.classList.add("dragover");
  else if (ev.payload.type === "leave") dropzone.classList.remove("dragover");
  else if (ev.payload.type === "drop") {
    dropzone.classList.remove("dragover");
    if (ev.payload.paths?.length) addPaths(ev.payload.paths);
  }
});

document.querySelectorAll(".seg button").forEach((b) => {
  if (b.dataset.mode === mode) {
    document.querySelector(".seg .active")?.classList.remove("active");
    b.classList.add("active");
  }
  b.addEventListener("click", () => {
    document.querySelector(".seg .active")?.classList.remove("active");
    b.classList.add("active");
    mode = b.dataset.mode;
    localStorage.setItem("squish.mode", mode);
  });
});

$("#dest-btn").addEventListener("click", async () => {
  const dir = await open({ directory: true });
  if (dir) {
    outDir = dir;
    $("#dest-label").textContent = dir;
    $("#dest-reset").classList.remove("hidden");
  }
});
$("#dest-reset").addEventListener("click", () => {
  outDir = null;
  $("#dest-label").textContent = "Next to originals";
  $("#dest-reset").classList.add("hidden");
});

cancelBtn.addEventListener("click", () => invoke("cancel_all"));

clearBtn.addEventListener("click", () => {
  if (running) return;
  files.clear();
  listEl.innerHTML = "";
  refreshChrome();
});

const appWindow = getCurrentWindow();

async function syncMaxIcon() {
  let maximized = false;
  try { maximized = await appWindow.isMaximized(); } catch {}
  const btn = $("#btn-max");
  btn.querySelector("use").setAttribute("href", maximized ? "#i-restore" : "#i-max");
  btn.title = maximized ? "Restore" : "Maximize";
  btn.setAttribute("aria-label", btn.title);
}

$("#btn-min").addEventListener("click", () => appWindow.minimize());
$("#btn-max").addEventListener("click", async () => {
  await appWindow.toggleMaximize();
  syncMaxIcon();
});
$("#btn-close").addEventListener("click", () => appWindow.close());

document.querySelector(".titlebar").addEventListener("dblclick", async (e) => {
  if (e.target.closest(".winctl")) return;
  await appWindow.toggleMaximize();
  syncMaxIcon();
});

appWindow.onResized(syncMaxIcon);
syncMaxIcon();

refreshChrome();
