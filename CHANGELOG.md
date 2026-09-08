# 更新日志

本项目所有值得注意的变更都记录在此文件。

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。
版本号单一来源为根 `package.json`（`tauri.conf.json` 引用它，菜单/安装包版本自动跟随）。

## [0.0.1] - 2026-09-08

初版发布。

### 新增

- **桌宠状态指示**：常驻桌面的螃蟹小窗（无边框、透明背景、置顶、不进 Dock），由 Claude Code hooks 驱动，实时反映会话状态（空闲 / 思考中 / 执行中 / 等待授权等），等待授权时显示感叹号徽标
- **本地模型代理**：内置 `127.0.0.1:15721` HTTP 代理，Claude Code 与 Claude Desktop 共用，替代外部模型切换工具；支持 Anthropic / OpenAI 两种上游协议（OpenAI 走协议转换）
- **模型管理**：右键菜单模型列表（✓ 标记当前项，点击即切换）+ 独立管理小窗（增删改模型、保存前连通性「测试」按钮）
- **原生右键菜单**：系统级菜单（不受小窗尺寸裁剪），顶部以 macOS 原生绿点显示代理运行状态及端口，未启动时提供「重试启动代理」
- **首启自动化**：自动安装 hooks 到 `~/.claude/settings.json`、生成 `~/.claude/claude-pet/models.json`、注册 Claude Desktop 模型 profile

### 修复与健壮性

- hooks 信号失效时以 transcript 活跃度兜底，桌宠动画不僵死；`settings.json` hooks 丢失可自愈
- Sending / Waiting 状态桌宠定住不误播动画；权限批准后 alert 徽标正确解除
- Claude Desktop 模型选择器收敛为单条入口，代理特判替换为当前激活模型

[0.0.1]: https://github.com/pbstar/claude-pet/releases/tag/v0.0.1
