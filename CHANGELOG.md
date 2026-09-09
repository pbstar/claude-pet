# 更新日志

本项目所有值得注意的变更都记录在此文件。

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。
版本号单一来源为根 `package.json`（`tauri.conf.json` 引用它，菜单/安装包版本自动跟随）。

## [0.0.2] - 2026-09-09

### 修复

- openai 协议转换对齐 cc-switch 语义，修复一批流式故障：`[1m]` 后缀误发上游、工具块 start/stop 不配对、UTF-8 跨块截断断流、usage 缺失/重复、thinking 内容丢失等

### 重构

- 流式翻译状态机拆分到 `stream.rs`；清理无用状态包装与重复定义；收窄依赖 features

### 文档

- README 重写：补「已知限制」，分节说明工作原理；修正无依据的系统版本要求

## [0.0.1] - 2026-09-08

### 新增

- **桌宠状态指示**：常驻桌面的螃蟹小窗，由 Claude Code hooks 驱动，实时反映会话状态（空闲 / 思考中 / 执行中 / 等待授权等）
- **本地模型代理**：内置 `127.0.0.1:15721` HTTP 代理，Claude Code 与 Claude Desktop 共用；支持 Anthropic / OpenAI 两种上游协议（OpenAI 走协议转换）
- **模型管理**：右键菜单切换模型 + 独立管理小窗（增删改、连通性测试）
- **原生右键菜单**：系统级菜单，绿点显示代理运行状态，未启动时可重试
- **首启自动化**：自动安装 hooks、生成模型配置、注册 Claude Desktop profile

### 修复

- hooks 信号失效时以 transcript 活跃度兜底，桌宠动画不僵死
- 权限批准后徽标正确解除；Claude Desktop 模型选择器收敛为单条入口

[0.0.2]: https://github.com/pbstar/claude-pet/releases/tag/v0.0.2
[0.0.1]: https://github.com/pbstar/claude-pet/releases/tag/v0.0.1
