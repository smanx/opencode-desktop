# OpenCode Desktop

English | **[中文](README.md)**

OpenCode Desktop is a desktop launcher for [OpenCode](https://opencode.ai). Powered by Tauri 2, it detects and starts the local `opencode serve` service and opens its web interface inside the app window.

## Features

- **Fast**: native Tauri 2 implementation, small installer, low memory footprint.
- **Lightweight**: this project is only a launcher/entry point for OpenCode; it does not bundle OpenCode itself. Install OpenCode first:

  ```bash
  npm install -g opencode-ai
  ```

- **Auto-detection**: on startup it checks whether an opencode server is already running, on the default port (4096) or any other port.
- **Auto-start**: when no server is running, it finds and launches `opencode serve --port 4096` automatically.
- **In-app UI**: once the service is ready, the opencode web interface opens inside the app window.
- **Cross-platform**: works on Windows, macOS and Linux, and locates the `opencode` command correctly on each platform.
- **Clean exit**: on quit the app only stops the opencode process it started itself; user-started instances keep running.

## Requirements

- [Node.js](https://nodejs.org/) (20 or newer recommended)
- [Rust](https://www.rust-lang.org/) stable
- [Tauri 2 platform prerequisites](https://tauri.app/start/prerequisites/) (WebView2 on Windows, webkit2gtk on Linux, etc.)
- OpenCode installed globally (see above)

## Development

```bash
npm install
npm run tauri dev
```

## Build

```bash
npm run tauri build
```

### Auto release (GitHub Actions)

The [build-release.yml](.github/workflows/build-release.yml) workflow builds Windows, macOS and Linux installers on demand and creates a draft release.

### Auto verification (GitHub Actions)

[verify.yml](.github/workflows/verify.yml) runs end-to-end verification on Windows/macOS/Linux: installs `opencode-ai`, smoke-tests the opencode server ([smoke-opencode-web.ps1](.github/scripts/smoke-opencode-web.ps1)), builds and silently installs the app, launches it and confirms it auto-starts the opencode server, takes screenshots, and outputs a PASS/FAIL verdict.

## How it works

1. On startup the app calls the `check_opencode` command to probe the service.
2. If opencode is already running (default port 4096, or any port serving the opencode web UI), it navigates there.
3. Otherwise it locates the `opencode` command (PATH first, then common install dirs) and runs `opencode serve --port 4096`, polling until ready.
4. Once ready it opens the web UI in the window; on exit it kills the process tree it spawned (and only that one).

> Note: it starts `opencode serve` (a headless server with an embedded web UI) rather than `opencode web`, which would pop the system browser.

### Server password

If you set the `OPENCODE_SERVER_PASSWORD` environment variable (or enabled server auth via opencode config), this app automatically supplies the password when opening the UI, so no extra login prompt appears. Without a password, the UI opens directly.

## Logs

opencode output is written to `opencode-web.log` in the system app-log directory. Check it if startup fails or times out.

## License

MIT, see [LICENSE](LICENSE).
