const { invoke } = window.__TAURI__.core;

const statusEl = document.getElementById("status");
const spinnerEl = document.getElementById("spinner");
const urlEl = document.getElementById("url");
const errorBox = document.getElementById("error");
const errorText = document.getElementById("error-text");
const retryBtn = document.getElementById("retry");

let lastStatus = null;

function setStatus(text, busy) {
  statusEl.textContent = text;
  spinnerEl.hidden = !busy;
}

function showError(text) {
  errorText.textContent = text;
  errorBox.hidden = false;
  spinnerEl.hidden = true;
}

async function probe() {
  try {
    lastStatus = await invoke("check_opencode");
    return lastStatus.running === true;
  } catch {
    return false;
  }
}

async function waitReady(timeoutMs) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    if (await probe()) return true;
    await new Promise((r) => setTimeout(r, 1000));
  }
  return false;
}

function displayUrl(u) {
  return u.replace(/\/\/[^/@]+@/, "//");
}

function open(u) {
  setStatus("opencode 已就绪，正在打开界面…", true);
  urlEl.hidden = false;
  urlEl.textContent = "正在打开：" + displayUrl(u);
  setTimeout(() => {
    window.location.replace(u);
  }, 300);
}

async function boot() {
  setStatus("正在检测 opencode…", true);
  errorBox.hidden = true;

  let status;
  try {
    status = await invoke("check_opencode");
  } catch (e) {
    showError("检测失败：" + (e && e.message ? e.message : e));
    return;
  }

  if (status.running) {
    open(status.url);
    return;
  }

  if (!status.installed) {
    showError("本机未检测到 opencode 命令。请先安装 opencode：npm install -g opencode-ai，安装完成后点击重试。");
    return;
  }

  setStatus("正在启动 opencode…", true);
  try {
    await invoke("start_opencode");
  } catch (e) {
    const msg = typeof e === "string" ? e : e && e.message ? e.message : String(e);
    showError(msg);
    return;
  }

  if (await waitReady(90000)) {
    open(lastStatus.url);
  } else {
    showError("opencode 启动超时，请检查日志后重试。");
  }
}

retryBtn.addEventListener("click", boot);
boot();
