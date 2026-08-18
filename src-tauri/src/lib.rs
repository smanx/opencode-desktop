use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Serialize;
use sysinfo::{ProcessRefreshKind, System, UpdateKind};
use tauri::{AppHandle, Manager, State};

const OPENCODE_HOST: &str = "127.0.0.1";
const OPENCODE_PORT: u16 = 4096;

const INSTALL_HINT: &str =
    "本机未检测到 opencode 命令。请先安装 opencode（npm install -g opencode-ai），安装完成后点击重试。";

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

struct OpenCodeState {
    /// PID of the opencode instance this app spawned (None when the app did
    /// not start opencode, i.e. a user-started instance is being used).
    spawned: Mutex<Option<u32>>,
}

#[derive(Serialize)]
struct OpenCodeStatus {
    running: bool,
    installed: bool,
    url: String,
}

/// Percent-encode a URL userinfo component (username or password), keeping
/// only the RFC 3986 unreserved characters.
fn url_userinfo(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `user:password@` userinfo for a navigation URL, when
/// `OPENCODE_SERVER_PASSWORD` is configured. Embedding the credentials in the
/// URL lets the WebView answer the server's Basic auth challenge itself, so
/// no modal auth dialog appears (which would also hang WebView teardown on
/// close). Empty when no password is set.
fn basic_auth_url() -> String {
    let password = match std::env::var("OPENCODE_SERVER_PASSWORD") {
        Ok(p) if !p.is_empty() => p,
        _ => return String::new(),
    };
    let username = std::env::var("OPENCODE_SERVER_USERNAME")
        .unwrap_or_else(|_| "opencode".to_string());
    format!("{}:{}@", url_userinfo(&username), url_userinfo(&password))
}

fn opencode_url(port: Option<u16>) -> String {
    let auth = basic_auth_url();
    format!(
        "http://{auth}{OPENCODE_HOST}:{}",
        port.unwrap_or(OPENCODE_PORT)
    )
}

fn opencode_in_dir(dir: &std::path::Path) -> Option<PathBuf> {
    let base = dir.join("opencode");
    if base.is_file() {
        return Some(base);
    }
    #[cfg(windows)]
    {
        for ext in ["exe", "cmd", "bat", "com"] {
            let candidate = base.with_extension(ext);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Candidate directories where `opencode` may be installed on macOS/Linux.
/// GUI-launched apps (Finder / `.desktop` / `open`) do not always inherit the
/// login shell's PATH, so we scan a set of common locations: npm/pnpm/bun
/// global bins, cargo, node version managers (nvm/volta/asdf/fnm/mise), and
/// the system bin dirs.
#[cfg(not(windows))]
fn common_bin_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/bin"),
    ];
    if let Ok(home) = std::env::var("HOME") {
        let home = PathBuf::from(home);
        dirs.extend([
            home.join(".npm-global/bin"),
            home.join(".local/bin"),
            home.join(".cargo/bin"),
            home.join(".bun/bin"),
            home.join(".volta/bin"),
            home.join(".asdf/shims"),
            home.join(".local/share/mise/shims"),
            home.join(".local/share/fnm"),
        ]);
        // nvm keeps one `bin` dir per installed Node version, e.g.
        // ~/.nvm/versions/node/v20.11.0/bin.
        if let Ok(versions) = std::fs::read_dir(home.join(".nvm/versions/node")) {
            for entry in versions.flatten() {
                dirs.push(entry.path().join("bin"));
            }
        }
    }
    dirs
}

/// Resolve the `opencode` command so the app works on Windows/macOS/Linux no
/// matter how opencode was installed. Searches PATH first, then a set of
/// common install locations (GUI-launched apps on macOS/Linux do not always
/// inherit the login shell's PATH).
fn find_opencode() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            if let Some(p) = opencode_in_dir(&dir) {
                return Some(p);
            }
        }
    }
    #[cfg(not(windows))]
    {
        for dir in common_bin_dirs() {
            if let Some(p) = opencode_in_dir(&dir) {
                return Some(p);
            }
        }
    }
    None
}

fn opencode_installed() -> bool {
    find_opencode().is_some()
}

/// True when a (lowercased) command-line token looks like the `opencode`
/// executable: the bare `opencode`, an `opencode`/`opencode.exe` binary
/// invoked through its absolute path, or a binary under the `opencode-ai`
/// package. The app's own process (`opencode-desktop.exe`) never matches.
fn is_opencode_token(token: &str) -> bool {
    if token == "opencode" {
        return true;
    }
    if let Some(name) = std::path::Path::new(token).file_name().and_then(|f| f.to_str()) {
        let stem = name
            .strip_suffix(".exe")
            .or_else(|| name.strip_suffix(".cmd"))
            .or_else(|| name.strip_suffix(".bat"))
            .or_else(|| name.strip_suffix(".com"))
            .unwrap_or(name);
        if stem == "opencode" {
            return true;
        }
    }
    token.contains("opencode-ai")
        || token.contains(r"\opencode\")
        || token.contains("/opencode/")
        || token.contains(r"\opencode/")
        || token.contains(r"/opencode\")
}

/// Enumerate processes whose command line looks like a running opencode
/// server instance (e.g. `.../opencode serve --port 8080` or
/// `opencode web --port 4096`). This is what lets the app find an opencode
/// that the user started on any port. Matching is strict on purpose: a bare
/// token "serve" or "web" plus an opencode-looking path or command, so the
/// app's own process and its WebView2 helpers never match.
fn find_opencode_processes() -> Vec<u32> {
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessRefreshKind::new().with_cmd(UpdateKind::Always));
    let mut pids = Vec::new();
    for (pid, process) in sys.processes() {
        let cmd: Vec<String> = process.cmd().iter().map(|s| s.to_lowercase()).collect();
        let is_opencode = cmd.iter().any(|t| is_opencode_token(t));
        let is_server = cmd.iter().any(|t| t == "serve" || t == "web");
        if is_opencode && is_server {
            pids.push(pid.as_u32());
        }
    }
    pids.sort_unstable();
    pids
}

fn parse_port(addr: &str) -> Option<u16> {
    addr.rsplit(':').next()?.parse::<u16>().ok()
}

/// /proc/net/tcp uses hex-encoded ports (e.g. `0100007F:0C08` -> 3080).
#[allow(dead_code)]
fn parse_hex_port(addr: &str) -> Option<u16> {
    u16::from_str_radix(addr.rsplit(':').next()?, 16).ok()
}

/// Parse `netstat -ano` output (Windows) and return the listening TCP ports
/// owned by `pid`.
#[allow(dead_code)]
fn ports_from_netstat(out: &str, pid: u32) -> Vec<u16> {
    let mut ports = Vec::new();
    for line in out.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() >= 5 && f[0].starts_with("TCP") && f[3] == "LISTENING" {
            if let Ok(p) = f[4].parse::<u32>() {
                if p == pid {
                    if let Some(port) = parse_port(f[1]) {
                        ports.push(port);
                    }
                }
            }
        }
    }
    ports
}

/// Parse `/proc/<pid>/net/tcp` (or tcp6) output (Linux). `inodes` is the set
/// of socket inodes held by the process's fds; only those rows are returned.
#[allow(dead_code)]
fn ports_from_proc_tcp(tcp: &str, inodes: &[u64]) -> Vec<u16> {
    let mut ports = Vec::new();
    for line in tcp.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() >= 10 && f[3] == "0A" {
            if let Ok(inode) = f[9].parse::<u64>() {
                if inodes.contains(&inode) {
                    if let Some(port) = parse_hex_port(f[1]) {
                        ports.push(port);
                    }
                }
            }
        }
    }
    ports
}

/// Parse `lsof -nP -iTCP -sTCP:LISTEN -a -p <pid>` output (macOS) and return
/// the listening TCP ports owned by `pid`.
#[allow(dead_code)]
fn ports_from_lsof(out: &str, pid: u32) -> Vec<u16> {
    let mut ports = Vec::new();
    for line in out.lines() {
        let idx = match line.find("(LISTEN)") {
            Some(i) => i,
            None => continue,
        };
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 2 || f[1].parse::<u32>().ok() != Some(pid) {
            continue;
        }
        let before = &line[..idx];
        if let Some(addr) = before.trim_end().rsplit(' ').find(|s| !s.is_empty()) {
            if let Some(port) = parse_port(addr) {
                ports.push(port);
            }
        }
    }
    ports
}

/// Listening TCP ports owned by `pid`, discovered per platform.
fn listening_ports_of(pid: u32) -> Vec<u16> {
    #[cfg(windows)]
    {
        let out = Command::new("netstat")
            .args(["-ano"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        ports_from_netstat(&out, pid)
    }

    #[cfg(target_os = "linux")]
    {
        use std::fs;
        let mut inodes = Vec::new();
        if let Ok(entries) = fs::read_dir(format!("/proc/{pid}/fd")) {
            for entry in entries.flatten() {
                if let Ok(target) = fs::read_link(entry.path()) {
                    let s = target.to_string_lossy();
                    if let Some(num) = s
                        .strip_prefix("socket:[")
                        .and_then(|r| r.strip_suffix(']'))
                    {
                        if let Ok(inode) = num.parse::<u64>() {
                            inodes.push(inode);
                        }
                    }
                }
            }
        }
        let mut ports = Vec::new();
        for file in [format!("/proc/{pid}/net/tcp"), format!("/proc/{pid}/net/tcp6")] {
            if let Ok(content) = fs::read_to_string(file) {
                ports.extend(ports_from_proc_tcp(&content, &inodes));
            }
        }
        ports
    }

    #[cfg(target_os = "macos")]
    {
        let out = Command::new("lsof")
            .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-a", "-p", &pid.to_string()])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        ports_from_lsof(&out, pid)
    }

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        Vec::new()
    }
}

fn parse_status(head: &str) -> Option<u16> {
    let line = head.lines().next()?;
    let mut parts = line.split_whitespace();
    parts.next()?; // HTTP/1.x
    parts.next()?.parse::<u16>().ok()
}

/// Build an HTTP Basic auth header from the opencode server env vars, when a
/// password is configured. The server this app spawns inherits the same
/// environment, so the same credentials probe it; user-started instances
/// usually inherit them too.
fn basic_auth_header() -> Option<String> {
    let password = std::env::var("OPENCODE_SERVER_PASSWORD").ok()?;
    let username = std::env::var("OPENCODE_SERVER_USERNAME")
        .unwrap_or_else(|_| "opencode".to_string());
    let token = STANDARD.encode(format!("{username}:{password}"));
    Some(format!("Authorization: Basic {token}\r\n"))
}

/// GET `path` on `port`; returns the HTTP status code and the first bytes of
/// the body if a response arrived.
fn fetch_head(port: u16, path: &str) -> Option<(u16, Vec<u8>)> {
    let addr: SocketAddr = format!("{OPENCODE_HOST}:{port}").parse().ok()?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(800)).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(800))).ok()?;
    let auth = basic_auth_header().unwrap_or_default();
    let req = format!(
        "GET {path} HTTP/1.0\r\nHost: {OPENCODE_HOST}:{port}\r\n{auth}\r\n"
    );
    stream.write_all(req.as_bytes()).ok()?;
    // Read until the headers plus a chunk of the body arrive, so a slow or
    // split first write can't hide the opencode markers.
    let mut buf = Vec::with_capacity(16384);
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() >= 16384 {
                    break;
                }
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    if buf.len() - pos - 4 >= 1024 {
                        break;
                    }
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(_) => break,
        }
    }
    if buf.is_empty() {
        return None;
    }
    let code = parse_status(&String::from_utf8_lossy(&buf))?;
    Some((code, buf))
}

/// True when `port` answers like an opencode server. The `/` route serves the
/// embedded web UI (its index.html carries an "OpenCode" marker);
/// `/global/health` is a definitive opencode API endpoint. Requests are sent
/// with the inherited `OPENCODE_SERVER_PASSWORD` when set, so protected
/// servers answer 200; a Basic-auth challenge (401/403) is also accepted as
/// opencode when it appears on a port tied to an opencode process.
fn is_opencode_server(port: u16) -> bool {
    for path in ["/", "/global/health"] {
        if let Some((code, body)) = fetch_head(port, path) {
            let text = String::from_utf8_lossy(&body).to_lowercase();
            if (200..400).contains(&code) {
                if text.contains("opencode") || text.contains("healthy") {
                    return true;
                }
            }
            if (code == 401 || code == 403) && text.contains("www-authenticate: basic") {
                return true;
            }
        }
    }
    false
}

/// Resolve the port of a running opencode server. Prefers the default port
/// (4096), then any port a running opencode process is serving. Returns
/// `None` when nothing is serving opencode.
fn discover_opencode() -> Option<u16> {
    if is_opencode_server(OPENCODE_PORT) {
        return Some(OPENCODE_PORT);
    }
    for pid in find_opencode_processes() {
        for port in listening_ports_of(pid) {
            if port != OPENCODE_PORT && is_opencode_server(port) {
                return Some(port);
            }
        }
    }
    None
}

fn spawn_opencode(app: &AppHandle) -> Result<u32, String> {
    let opencode_path = find_opencode().ok_or_else(|| INSTALL_HINT.to_string())?;

    let log_dir = app.path().app_log_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&log_dir).map_err(|e| e.to_string())?;

    let log_path = log_dir.join("opencode-web.log");
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("cannot open log {}: {e}", log_path.display()))?;

    // Windows npm shims (opencode.cmd / .bat) must run through `cmd /C`; a
    // real `opencode.exe` can be spawned directly. Other platforms exec the
    // resolved self-contained binary directly.
    let (program, args): (String, Vec<String>) = if cfg!(windows) {
        let ext = opencode_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        if ext == "exe" {
            (
                opencode_path.to_string_lossy().into_owned(),
                vec!["serve".into(), "--port".into(), OPENCODE_PORT.to_string()],
            )
        } else {
            (
                "cmd".into(),
                vec![
                    "/C".into(),
                    "opencode".into(),
                    "serve".into(),
                    "--port".into(),
                    OPENCODE_PORT.to_string(),
                ],
            )
        }
    } else {
        (
            opencode_path.to_string_lossy().into_owned(),
            vec!["serve".into(), "--port".into(), OPENCODE_PORT.to_string()],
        )
    };

    let mut cmd = Command::new(&program);
    cmd.args(&args)
        .stdout(Stdio::from(
            log_file
                .try_clone()
                .map_err(|e| format!("cannot clone log handle: {e}"))?,
        ))
        .stderr(Stdio::from(log_file));

    // opencode ships as a self-contained binary, but plugins / git tooling it
    // launches may rely on node & friends. Prepend likely bin dirs to PATH on
    // unix where GUI-launched apps inherit a minimal PATH.
    #[cfg(unix)]
    {
        let mut dirs: Vec<String> = Vec::new();
        if let Some(parent) = opencode_path.parent() {
            dirs.push(parent.to_string_lossy().into_owned());
        }
        dirs.extend(
            common_bin_dirs()
                .iter()
                .map(|d| d.to_string_lossy().into_owned()),
        );
        if let Ok(inherited) = std::env::var("PATH") {
            dirs.push(inherited);
        }
        cmd.env("PATH", dirs.join(":"));
    }

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);

    // Put opencode in its own process group so we can kill the whole tree on
    // exit.
    #[cfg(unix)]
    cmd.process_group(0);

    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to start `{program} {args:?}`: {e}"))?;
    Ok(child.id())
}

/// Kill the opencode instance this app spawned (the whole process tree). Used
/// on app exit; only called when the app started opencode itself.
fn kill_opencode_tree(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }

    #[cfg(unix)]
    {
        // opencode was spawned with process_group(0), so its group id == pid.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
        std::thread::sleep(Duration::from_millis(800));
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }

    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
    }
}

fn ensure_started(app: &AppHandle, state: &OpenCodeState) -> Result<Option<u32>, String> {
    if discover_opencode().is_some() {
        return Ok(None);
    }
    if !opencode_installed() {
        return Err(INSTALL_HINT.to_string());
    }
    // An opencode process exists but is not serving yet (still booting): wait
    // for it instead of spawning a duplicate on port 4096.
    if !find_opencode_processes().is_empty() {
        return Ok(None);
    }
    let mut spawned = state.spawned.lock().unwrap();
    if spawned.is_some() {
        return Ok(None);
    }
    let pid = spawn_opencode(app)?;
    *spawned = Some(pid);
    Ok(Some(pid))
}

#[tauri::command]
fn check_opencode() -> OpenCodeStatus {
    let port = discover_opencode();
    OpenCodeStatus {
        running: port.is_some(),
        installed: opencode_installed(),
        url: opencode_url(port),
    }
}

#[tauri::command]
fn start_opencode(app: AppHandle, state: State<'_, OpenCodeState>) -> Result<OpenCodeStatus, String> {
    let port = discover_opencode();
    if let Some(p) = port {
        return Ok(OpenCodeStatus {
            running: true,
            installed: opencode_installed(),
            url: opencode_url(Some(p)),
        });
    }
    ensure_started(&app, &state)?;
    let p = discover_opencode();
    Ok(OpenCodeStatus {
        running: p.is_some(),
        installed: opencode_installed(),
        url: opencode_url(p),
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.unminimize();
                let _ = win.show();
                let _ = win.set_focus();
            }
        }))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(OpenCodeState {
            spawned: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![check_opencode, start_opencode])
        .on_window_event(|window, event| {
            // Kill the spawned opencode early, before WebView teardown, so it
            // is never orphaned even if the WebView hangs while closing.
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                let handle = window.app_handle().clone();
                let state = handle.state::<OpenCodeState>();
                let spawned_pid = *state.spawned.lock().unwrap();
                if let Some(pid) = spawned_pid {
                    kill_opencode_tree(pid);
                }
                // Watchdog: WebView2 teardown can occasionally hang after a
                // close request. If the process has not exited within a few
                // seconds, force-exit so no zombie remains.
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(6));
                    if let Some(pid) = spawned_pid {
                        kill_opencode_tree(pid);
                    }
                    std::process::exit(0);
                });
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let state = handle.state::<OpenCodeState>();
                if let Err(e) = ensure_started(&handle, &state) {
                    eprintln!("opencode auto-start failed: {e}");
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            // Only stop opencode when this app spawned it; a user-started
            // instance keeps running after the app closes.
            let spawned_pid = {
                let state = handle.state::<OpenCodeState>();
                let guard = state.spawned.lock().unwrap();
                *guard
            };
            if let Some(pid) = spawned_pid {
                kill_opencode_tree(pid);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_port_basic() {
        assert_eq!(parse_port("127.0.0.1:4096"), Some(4096));
        assert_eq!(parse_port("[::1]:4097"), Some(4097));
        assert_eq!(parse_port("*:8090"), Some(8090));
        assert_eq!(parse_port("no-port"), None);
    }

    #[test]
    fn parse_status_basic() {
        assert_eq!(parse_status("HTTP/1.0 200 OK"), Some(200));
        assert_eq!(parse_status("HTTP/1.1 404 Not Found"), Some(404));
        assert_eq!(parse_status("garbage"), None);
    }

    #[test]
    fn netstat_windows_parse() {
        let sample = "\
Active Connections

  Proto  Local Address          Foreign Address        State           PID
  TCP    127.0.0.1:4096         0.0.0.0:0              LISTENING       38316
  TCP    127.0.0.1:4097         0.0.0.0:0              LISTENING       38316
  TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       1204
  UDP    0.0.0.0:1900           *:*                                   544
";
        assert_eq!(ports_from_netstat(sample, 38316), vec![4096, 4097]);
        assert_eq!(ports_from_netstat(sample, 1204), vec![135]);
    }

    #[test]
    fn proc_tcp_linux_parse() {
        let sample = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1000 00000000:0000 0A 00000000:00000000 000:00000 00000000     0        0 12345 1 0000000000000000 100 0 0 10 0
   1: 0100007F:1001 00000000:0000 0A 00000000:00000000 000:00000 00000000     0        0 12346 1 0000000000000000 100 0 0 10 0
   2: 0100007F:D204 00000000:0000 01 00000000:00000000 000:00000 00000000     0        0 12347 1 0000000000000000 100 0 0 10 0
";
        assert_eq!(ports_from_proc_tcp(sample, &[12345]), vec![4096]);
        assert_eq!(ports_from_proc_tcp(sample, &[12346]), vec![4097]);
        // non-LISTEN row (state 01) must be ignored
        assert_eq!(ports_from_proc_tcp(sample, &[12347]), Vec::<u16>::new());
    }

    #[test]
    fn lsof_macos_parse() {
        let sample = "\
COMMAND PID   USER  FD   TYPE DEVICE SIZE/OFF NODE NAME
node    38316 cc    30u  IPv4 0x1   0t0     TCP 127.0.0.1:4096 (LISTEN)
node    38316 cc    31u  IPv6 0x2   0t0     TCP *:4097 (LISTEN)
node    999   cc    10u  IPv4 0x3   0t0     TCP 127.0.0.1:8443 (LISTEN)
";
        assert_eq!(ports_from_lsof(sample, 38316), vec![4096, 4097]);
        assert_eq!(ports_from_lsof(sample, 999), vec![8443]);
    }

    #[test]
    fn opencode_token_matches() {
        assert!(is_opencode_token("opencode"));
        assert!(is_opencode_token("opencode.exe"));
        assert!(is_opencode_token("opencode.cmd"));
        assert!(is_opencode_token("/usr/local/bin/opencode"));
        assert!(is_opencode_token("/home/me/.cargo/bin/opencode"));
        assert!(is_opencode_token("C:\\Users\\me\\AppData\\Roaming\\npm\\opencode.exe"));
        assert!(is_opencode_token(
            "C:\\Users\\me\\AppData\\Roaming\\npm\\node_modules\\opencode-ai\\bin\\opencode.exe"
        ));
        assert!(!is_opencode_token("opencode-desktop.exe"));
        assert!(!is_opencode_token("node"));
        assert!(!is_opencode_token("/usr/bin/bash"));
        assert!(!is_opencode_token("--webview-exe-name=msedgewebview2.exe"));
        assert!(!is_opencode_token("/opt/dashboard/bin/web"));
    }

    #[test]
    fn basic_auth_header_builds() {
        unsafe {
            std::env::set_var("OPENCODE_SERVER_USERNAME", "opencode");
            std::env::set_var("OPENCODE_SERVER_PASSWORD", "hunter2");
        }
        assert_eq!(
            basic_auth_header(),
            Some("Authorization: Basic b3BlbmNvZGU6aHVudGVyMg==\r\n".to_string())
        );
        // default username when only the password is set
        unsafe {
            std::env::remove_var("OPENCODE_SERVER_USERNAME");
        }
        assert_eq!(
            basic_auth_header(),
            Some("Authorization: Basic b3BlbmNvZGU6aHVudGVyMg==\r\n".to_string())
        );
        // no password configured -> no header
        unsafe {
            std::env::remove_var("OPENCODE_SERVER_PASSWORD");
        }
        assert_eq!(basic_auth_header(), None);
    }

    #[test]
    fn url_userinfo_encodes() {
        assert_eq!(url_userinfo("opencode"), "opencode");
        assert_eq!(
            url_userinfo("0514ac35-0b75-4d42-b243-0410b0a1fe1a"),
            "0514ac35-0b75-4d42-b243-0410b0a1fe1a"
        );
        assert_eq!(url_userinfo("p@ss:w/rd"), "p%40ss%3Aw%2Frd");
    }

    #[test]
    fn basic_auth_url_builds() {
        unsafe {
            std::env::set_var("OPENCODE_SERVER_USERNAME", "opencode");
            std::env::set_var("OPENCODE_SERVER_PASSWORD", "hunter2");
        }
        assert_eq!(basic_auth_url(), "opencode:hunter2@");
        assert_eq!(
            opencode_url(Some(4096)),
            "http://opencode:hunter2@127.0.0.1:4096"
        );
        // no password -> plain URL
        unsafe {
            std::env::remove_var("OPENCODE_SERVER_PASSWORD");
        }
        assert_eq!(basic_auth_url(), "");
        assert_eq!(opencode_url(Some(4096)), "http://127.0.0.1:4096");
        // empty password -> plain URL
        unsafe {
            std::env::set_var("OPENCODE_SERVER_PASSWORD", "");
        }
        assert_eq!(basic_auth_url(), "");
    }
}
