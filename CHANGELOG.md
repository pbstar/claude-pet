# 更新日志

本项目所有值得注意的变更都记录在此文件。

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。
版本号单一来源为根 `package.json`（`tauri.conf.json` 引用它，菜单/安装包版本自动跟随）。

## [0.0.3] - 2026-09-09

### 修复

- **黄灯闪跳**：并行工具/子代理触发的 `PreToolUse`/`PostToolUse` 会覆盖尚未解除的 `permission` 状态，桌宠在 alert 与 walking 之间来回跳。hook 侧加锁串行化「读旧状态 → 判定 → 写新状态」，并在权限待批准期间丢弃工作态写入（判据与 TS 端解冻逻辑一致：transcript 未推进即权限仍未解除）
- `notify` 改按结构化 `notification_type` 判定权限通知；原先全文匹配 `permission/approve/allow`，会被通知文案或路径误命中
- **压缩摘要的内部标签泄漏到正文**：上下文压缩时 Claude Code 要求模型把思考写进 `<analysis>`、总结写进 `<summary>`，官方链路由客户端自己剥离这两个标签；走 openai 转换时原文被当普通 text 块透传，标签直接显示在输出里。新增 `TagSanitizer`，语义与客户端 `ihg()` 一致——丢弃 `analysis` 段、`summary` 只留正文，流式分片切断标签也能正确识别（流式与非流式两条路径均生效）

### 优化

- 报警迟滞：进入 alert 立即生效，离开 alert 需连续两拍确认，吸收残余信号抖动
- `read_sessions` 先取 transcript mtime 再决定是否读尾部，陈旧会话不再每轮白读 8KB

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

[0.0.3]: https://github.com/pbstar/claude-pet/releases/tag/v0.0.3
[0.0.2]: https://github.com/pbstar/claude-pet/releases/tag/v0.0.2
[0.0.1]: https://github.com/pbstar/claude-pet/releases/tag/v0.0.1
