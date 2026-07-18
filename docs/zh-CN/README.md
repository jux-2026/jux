# Jux

[English](../../README.md) | [简体中文](README.md)

> [!WARNING]
> Jux 目前仍处于早期开发阶段。大量规划中的功能尚未完成，现有行为和接口也可能随时调整，
> 项目尚未完成生产环境所需的安全加固。**请勿将 Jux 用于生产环境。**

Jux 是一个开源、面向安全的编程 Agent，目标是通过本地运行时和多种客户端，为 AI
辅助软件开发提供可审计、可控和可扩展的能力。

本仓库是 Jux 的 Agent 侧 Rust monorepo，目前主要开发运行时基础、命令行界面和终端
用户界面。

## 项目状态

Jux 目前是一个持续开发中的项目，还不是完整、稳定的产品。现有实现适合开发、实验和
早期反馈，不适合生产任务或包含敏感信息的代码仓库。

当前项目尚未提供公开的产品说明文档。产品理念、使用场景和完整用户流程将在后续版本中
补充。在此之前，本 README 只介绍仓库中已经存在的能力。

## 当前能力

- 基于 Rust 的本地 Agent 运行时和 CLI。
- 基于 Ratatui 的终端用户界面。
- Workspace、Session、Run 和 Step 生命周期管理。
- 在工作区 `.jux` 目录中使用 SQLite 保存本地状态。
- 通过 DeepSeek 模型提供方执行多轮 LLM 任务。
- 带运行时策略检查的原生、Lua 和 WASM 工具。
- Skill 发现、显式或模型选择调用、独立上下文记录和恢复。
- 可持久化并恢复的人工澄清与确认流程。
- TUI 中的会话时间线、任务取消和代码变更审查。
- 仍在持续完善的并行任务、Context 管理、流式事件和 Plan 模式基础能力。

以上列表不代表稳定性承诺。部分能力仍未完整实现，并可能随着运行时演进而重新设计。

## 文档

英文是项目的默认文档语言。其他语言的翻译存放在 `docs/` 下各自的语言目录中。当前尚未
提供公开的产品说明文档，后续将在产品定义逐渐完善后补充。

## 仓库结构

```text
jux
├── crates
│   ├── jux-core
│   └── jux-cli
├── docs
│   └── zh-CN
├── AGENTS.md
├── Cargo.toml
├── Makefile
└── README.md
```

- `crates/jux-core` 包含核心领域模型和运行时行为。
- `crates/jux-cli` 包含 `jux` 可执行程序、CLI 适配层和 TUI 客户端。

## 安装

GitHub Releases 当前提供 Apple Silicon macOS、x86_64 Linux 和 x86_64
Windows 的预编译程序。这些安装包属于开发预览版本，同样不建议用于生产环境。

在 macOS 或 Linux 上使用 Shell 安装：

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/jux-2026/jux/releases/latest/download/jux-installer.sh | sh
```

在 Windows 上使用 PowerShell 安装：

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/jux-2026/jux/releases/latest/download/jux-installer.ps1 | iex"
```

也可以从 [GitHub Releases](https://github.com/jux-2026/jux/releases)
直接下载发布压缩包和 SHA-256 校验文件。

验证安装结果：

```sh
jux --version
```

检查新版本，并查看当前安装渠道对应的更新方式：

```sh
jux update --check
```

TUI 最多每 24 小时在后台检查一次更新。Jux 只会推荐适合当前渠道的更新命令，不会自动
执行该命令。

## 快速开始

配置 DeepSeek API Key，并在本地工作区运行一个任务：

```sh
export JUX_DEEPSEEK_API_KEY="..."
jux run "Explain this project" --workspace /path/to/workspace
```

可以通过顶层 `--output` 参数获得结构化输出：

```sh
jux --output json run "Explain this project"
jux --output yaml run "Explain this project"
```

Jux 会在所选工作区中初始化 `.jux/state.db`。使用能够执行命令或修改文件的工具时，请仔细
检查授权范围和生成的变更。

## 本地开发

Workspace 需要 Rust 1.91 或更高版本。请在仓库根目录执行：

```sh
make build
make test
make fmt
make lint
make quick-check
make check
```

`make quick-check` 执行格式和 lint 检查；`make check` 执行完整的本地质量检查，包括格式、
lint 和测试。

启用仓库内置的 Git Hooks：

```sh
git config core.hooksPath .githooks
```

## 许可证

Jux 使用 [MIT License](../../LICENSE) 开源。
