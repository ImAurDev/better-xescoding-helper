# 更好的学而思编程助手 (Better XES Coding Helper)

一个为学而思编程环境打造的本地辅助工具，提供多语言代码执行、包管理、资源下载和交互式终端等功能。

## 功能特性

- **多语言支持**：支持 Python、Golang、TypeScript 代码的执行与混合运行
- **包管理**：Python 包的搜索、安装、卸载，支持多镜像源切换
- **资源管理**：自动下载和管理课程相关资源文件
- **交互式终端**：基于 WebSocket 的 WebTTY，实时交互运行代码
- **历史记录**：保存代码运行历史，支持查看和回溯
- **危险代码检测**：运行前自动检测潜在危险操作并提示确认
- **运行时管理**：自动检测系统已安装的 Python、Go、Bun 路径，支持手动配置
- **嵌入式管理界面**：内嵌 Web 前端，无需额外部署即可通过浏览器管理

## 技术栈

- **后端**：Rust + Axum（异步 Web 框架）+ Tokio
- **前端**：React（编译后内嵌至二进制中）
- **通信**：HTTP REST API + WebSocket

## 系统要求

- Rust >= 1.80
- 操作系统：Windows / macOS / Linux
- 可选运行时（用于代码执行）：
  - Python（用于 Python 代码执行）
  - Golang（用于 Go 代码块执行）
  - Bun（用于 TypeScript 代码执行）

## 快速开始

### 编译运行

```bash
# 克隆仓库
git clone <repository-url>
cd xescoding-helper

# 编译并运行（开发模式）
cargo run

# 编译发行版本
cargo build --release
```

编译完成后，可执行文件位于 `target/release/xescoding_helper`（或 Windows 下的 `xescoding_helper.exe`）。

### 访问管理界面

程序启动后会自动查找可用端口并启动服务，默认端口范围为 `55820/55821`、`55825/55826` 等。启动成功后，在浏览器中打开对应的 HTTP 地址即可访问管理页面。

## 配置

可通过环境变量进行配置：

| 环境变量 | 说明 | 默认值 |
|---------|------|--------|
| `THONNY_PORT` | 基础端口 | `8000` |
| `THONNY_CACHE` | 缓存目录路径 | 系统本地数据目录下的 `xes-coding-helper` |

## API 概览

### 系统接口

- `GET /api/ping` — 服务存活检测
- `GET /api/version` — 获取版本号
- `GET /api/status` — 获取服务器状态/错误信息

### 运行时路径管理

- `GET /api/python-paths` / `POST /api/python-path` — Python 路径管理
- `GET /api/golang-paths` / `POST /api/golang-path` — Go 路径管理
- `GET /api/bun-paths` / `POST /api/bun-path` — Bun 路径管理

### 包管理

- `GET /package/search?name=xxx` — 搜索包
- `GET /package/local` — 获取已安装包列表
- `POST /package/install` — 安装包
- `POST /package/uninstall` — 卸载包
- `POST /package/cancel` — 取消安装
- `GET /package/mirrors` — 获取可用镜像列表
- `POST /package/mirror` — 切换镜像

### 历史记录

- `GET /api/history` — 获取运行历史
- `DELETE /api/history?id=xxx` — 删除单条记录
- `DELETE /api/history/clear` — 清空历史

### 资源处理

- `POST /api/path` — 处理资源 JSON 并获取本地路径

### WebSocket

- `WS /ws` — WebTTY 交互式终端连接

## 项目结构

```
.
├── Cargo.toml              # Rust 项目配置
├── LICENSE                 # MIT 许可证
├── src/
│   ├── main.rs             # 程序入口，服务启动
│   ├── config.rs           # 配置与端口定义
│   ├── state.rs            # 应用状态管理
│   ├── frontend.rs         # 内嵌前端资源（rust-embed）
│   ├── history.rs          # 运行历史记录管理
│   ├── logger.rs           # 日志初始化
│   ├── http/               # HTTP 服务与路由
│   │   ├── router.rs       # 路由构建
│   │   ├── handlers.rs     # 请求处理器
│   │   ├── cors.rs         # CORS 配置
│   │   └── port.rs         # 端口检测
│   ├── websocket/          # WebSocket 服务
│   │   ├── webtty.rs       # WebTTY 交互终端
│   │   └── mod.rs          # WebSocket 路由
│   ├── python/             # Python/多语言执行核心
│   │   ├── runner.rs       # 代码运行调度器
│   │   ├── exec_python.rs  # Python 执行
│   │   ├── exec_golang.rs  # Go 代码块执行
│   │   ├── exec_typescript.rs  # TS 代码块执行
│   │   ├── package_manager.rs  # 包管理器
│   │   ├── lib_list.rs     # 包列表管理
│   │   ├── danger.rs       # 危险代码检测
│   │   └── config.rs       # 运行时路径查找
│   ├── download/           # 资源下载管理
│   │   └── assets.rs       # 资源下载与缓存
│   └── utils/              # 工具函数
│       └── cache_cleaner.rs # 缓存清理
├── frontend/               # 前端源码（编译后内嵌）
│   ├── index.html
│   ├── frontend.js
│   └── styles.gen.css
└── target/                 # 编译输出
```

## 许可证

[MIT](LICENSE)

Copyright © 2026 极光