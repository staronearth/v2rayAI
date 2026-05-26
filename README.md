# v2rayAI

AI 驱动的 v2ray/xray 代理配置工具，基于 Tauri + Rust + React 构建。

## 下载安装

前往 [Releases](../../releases) 页面下载对应平台的安装包。

| 平台 | 安装包 | 说明 |
|------|--------|------|
| Windows | `.msi` 或 `.exe` | 推荐 `.msi` |
| macOS (Apple Silicon) | `.dmg` (aarch64) | M 系列芯片 |
| macOS (Intel) | `.dmg` (x86_64) | Intel 芯片 |
| Linux (Debian/Ubuntu) | `.deb` | |
| Linux (通用) | `.AppImage` | 无需安装，直接运行 |

### 系统要求

**Windows**
- Windows 10 / Windows 11
- 需要 [WebView2 Runtime](https://developer.microsoft.com/zh-cn/microsoft-edge/webview2/)（Windows 11 已内置；Windows 10 安装包会自动引导下载）

**macOS**
- macOS 10.15 (Catalina) 及以上
- 无需额外依赖

**Linux**
- 需要 `libwebkit2gtk-4.1`（Ubuntu 22.04+ 已内置）
- 如使用 `.AppImage`，需先赋予执行权限：
  ```bash
  chmod +x v2rayAI_*.AppImage
  ./v2rayAI_*.AppImage
  ```

## 已知问题

**Linux：启动时出现 EGL 错误**

在部分 Linux 系统（虚拟机、无独立显卡、某些 Wayland 环境）下可能出现：
```
Could not create default EGL display: EGL_BAD_PARAMETER. Aborting...
```
应用已内置兼容处理，若仍出现问题可手动设置环境变量：
```bash
WEBKIT_DISABLE_DMABUF_RENDERER=1 ./v2rayAI
```

## 开发

### 环境准备

- [Node.js](https://nodejs.org/) 20+
- [Rust](https://rustup.rs/) stable
- Linux 额外依赖：
  ```bash
  sudo apt-get install libwebkit2gtk-4.1-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
  ```

### 本地运行

```bash
npm install
npm run tauri dev
```

### 打包构建

```bash
npm run tauri build
```

## License

MIT
