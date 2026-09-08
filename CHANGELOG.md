# 更新日志

本项目所有值得注意的变更都记录在此文件。

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。
版本号单一来源为根 `package.json`（`tauri.conf.json` 引用它，菜单/安装包版本自动跟随）。

## [0.0.2] - 2026-09-08

### 变更

- **仅支持 Claude Desktop 的 Code tab**：移除 Claude Code CLI 专用通路——代理去掉 `/v1/messages` 与 `/v1/messages/count_tokens` 两个入口（Code tab 经 Desktop 注入的 host-creds 走 `/claude-desktop/*`），不再向 `settings.json` 注入 `ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN`
- 精简代码与注释：删除失效的 `docs/ccswitch-replacement.md` 章节引用与冗余分支

### 修复

- **openai 管道对齐 cc-switch 转换语义**，修掉一批真实故障源：
  - `[1m]` 后缀不再拼回模型名发给上游（仅用于驱动 `context-1m` beta 头）——上游普遍拒收该本地标记
  - 工具块等 id + name 都到齐才 `content_block_start`，先到的 args 缓存；流尾对未凑齐的块补 late-start，保证 start/stop 严格配对（此前首片只带 name 时会开出无名工具块）
  - SSE 字节流改为 `append_utf8_safe` 拼接，多字节 UTF-8 跨块截断不再整条流中断
  - 流式请求注入 `stream_options.include_usage`；`message_delta` 去重并延迟到流尾发出，携带完整 usage（此前每遇 finish_reason 就发一次）
  - `input_tokens` 扣除缓存命中（三桶互斥），补 `cache_read_input_tokens` / `cache_creation_input_tokens`
  - `reasoning_content` / `reasoning` → `thinking` 块（此前直接丢弃）
  - 请求侧补齐：o 系列模型改用 `max_completion_tokens`、thinking 预算映射 `reasoning_effort`、工具 schema 根节点补 `type: object`、system 首行 `x-anthropic-billing-header` 剥离

### 重构

- 流式翻译状态机从 `convert.rs` 拆到 `stream.rs`（含 10 项单元测试），`convert.rs` 只留请求侧与非流式响应转换
- 代理去掉从未使用的 axum state 包装（`ProxyState` / `AppStore` / `State` 提取器）——`store()` 本就是全局单例，直接改用一个 `static AtomicBool`
- `dirs_home()` 两处重复定义收敛到 `models.rs` 统一导出
- 收窄 `tokio` 依赖 features：`rt-multi-thread` + `net`（去掉未使用的 `macros` / `time`）
- `map_or(false, ..)` / `map_or(true, ..)` 改为 `is_some_and` / `is_none_or`

### 文档

- README 重写：补「已知限制」小节；卸载改为分步操作；「工作原理」按状态指示 / 模型代理 / 健壮性 / openai 兼容处理分节
- 修正 README 中无依据的「macOS 13+」——bundle 未声明 `minimumSystemVersion`，沿用 Tauri 默认值
- 补全目录结构缺失条目（`window-pos.ts`、`main.rs` 等）
- 删除 `menu.ts` / `manager.ts` 注释里残留的占位标记「（七）」

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

[0.0.2]: https://github.com/pbstar/claude-pet/releases/tag/v0.0.2
[0.0.1]: https://github.com/pbstar/claude-pet/releases/tag/v0.0.1
