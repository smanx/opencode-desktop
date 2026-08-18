# OpenCode Desktop

[English](README.en.md) | **中文**

OpenCode Desktop 是 [OpenCode](https://opencode.ai) 的桌面启动器。它由 Tauri 2 驱动，会自动检测、启动本机的 `opencode serve` 服务，并在应用窗口内打开其 Web 界面。

## 功能特性

- **极速体验**：基于 Tauri 2 原生实现，安装包体积小，运行内存占用低。
- **轻量入口**：本项目仅是 OpenCode 的启动器与入口，不包含 OpenCode 本体。请先自行安装 OpenCode：

  ```bash
  npm install -g opencode-ai
  ```

- **自动检测**：启动时检测本机是否已有 opencode 服务在运行，支持默认端口（4096）以及任意其他端口上的实例。
- **自动启动**：未检测到运行时，会自动查找并拉起 `opencode serve --port 4096`，无需手动敲命令。
- **界面跳转**：服务就绪后，在应用窗口内直接打开 opencode 的 Web 界面。
- **跨平台**：支持 Windows、macOS、Linux，并在多平台下都能正确找到 `opencode` 命令。
- **干净退出**：应用退出时只会结束由它自己启动的 opencode 进程，不会误杀用户自己启动的实例。

## 环境要求

- [Node.js](https://nodejs.org/)（建议 20 或更高）
- [Rust](https://www.rust-lang.org/) 稳定版
- [Tauri 2 平台依赖](https://tauri.app/start/prerequisites/)（各系统编译所需，如 Windows 的 WebView2、Linux 的 webkit2gtk 等）
- 已全局安装 OpenCode（见上文安装命令）

## 开发

```bash
# 安装依赖
npm install

# 启动开发模式（热重载）
npm run tauri dev
```

## 构建与打包

```bash
# 构建当前平台的安装包
npm run tauri build
```

各平台对应的打包目标由 `src-tauri/tauri.conf.json` 配置，目前默认使用 NSIS（Windows）。

### 自动发布（GitHub Actions）

仓库内置了 [build-release.yml](.github/workflows/build-release.yml) 工作流，可手动触发，会同时构建 Windows、macOS、Linux 三个平台的安装包，并生成草稿版本（draft release）。

### 自动验证（GitHub Actions）

[verify.yml](.github/workflows/verify.yml) 会在 Windows/macOS/Linux 上自动完成端到端验证：安装 `opencode-ai`、冒烟测试 opencode 服务（[smoke-opencode-web.ps1](.github/scripts/smoke-opencode-web.ps1)）、构建并静默安装桌面应用、启动应用并确认其自动拉起 opencode 服务、截图留档，最终给出 PASS/FAIL 结论。

## 工作原理

1. 应用启动后调用 `check_opencode` 命令检测服务状态。
2. 若发现 opencode 已在运行（默认端口 4096，或任意已监听且返回 opencode Web 页面特征的端口），直接跳转。
3. 否则查找 `opencode` 命令（优先 PATH，再探测常见安装目录），执行 `opencode serve --port 4096` 拉起服务并轮询等待就绪。
4. 服务就绪后窗口内打开 Web 界面；退出时若 opencode 是由本应用启动的，则一并结束其进程树。

> 说明：启动的是 `opencode serve`（headless 服务，自带内嵌 Web 界面），而不是 `opencode web`——后者会自动弹出系统浏览器。

### 关于服务密码

若你已设置 `OPENCODE_SERVER_PASSWORD` 环境变量（或通过 opencode 配置开启了服务认证），本应用会自动携带该密码访问服务，无需额外登录弹窗；未设置密码时则直接打开界面。

## 项目结构

```
opencode-desktop
├── src/                    # 前端页面（原生 HTML/CSS/JS）
│   ├── index.html
│   ├── main.js             # 调用 Tauri 命令并处理界面跳转
│   └── styles.css
├── src-tauri/              # Tauri 后端（Rust）
│   ├── src/
│   │   ├── main.rs
│   │   └── lib.rs          # 核心逻辑：检测、启动、端口探测
│   ├── capabilities/
│   └── tauri.conf.json     # 应用配置
├── doc/                    # 文档相关资源（界面截图等）
└── .github/workflows/      # CI 构建与发布
```

## 日志

应用启动 opencode 时会将输出写入系统应用日志目录下的 `opencode-web.log`。若启动失败或超时，可查看该日志排查问题。

## 许可

本项目基于 [MIT License](LICENSE) 开源。
