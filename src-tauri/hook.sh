#!/bin/bash
# claude-pet 状态 hook（极简，纯 shell，不依赖 node）
# 事件参数由安装器在 settings.json 里指定；Claude 的 hook JSON 从 stdin 传入。
# usage: hook.sh <thinking|tool|permission|done|notify|clean|start|denied>
#   - thinking/tool/permission/done：写会话状态文件
#   - notify：仅当通知是权限提示时才写 permission（过滤 idle_prompt 等无关通知）
#   - start：SessionStart 播种 idle，登记会话进程并覆盖 resume 可能残留的冻结状态
#   - denied：PermissionDenied，用户点了拒绝——该授权请求已终结，按工作中重新解析
#   - clean：删除会话状态（对应 SessionEnd）
set -u
DIR="$HOME/.claude/claude-pet"
mkdir -p "$DIR"

arg="${1:-}"
case "$arg" in
  thinking|tool|permission|done|notify|clean|start|denied) ;;
  # 无参/未知参数：不落盘，避免写出空状态污染状态目录
  *) exit 0 ;;
esac

# 读 stdin：分片读 + 单次 1s 超时。hook 的 stdin 若不关闭会永久阻塞，把整个 Claude 会话
# 一起拖死；这里最多等 1s 就放弃——宁可拿不到字段，也不能卡住会话
input=""
line=""
while IFS= read -r -t 1 line || [ -n "$line" ]; do
  input="$input$line"
done

# 取字段值：按出现顺序取第一个匹配。原先的贪婪 sed（.*"key"...）会命中最后一个，
# 工具入参里嵌套的同名 key 会让会话串号。$2 给定时从该文件读，否则读 stdin 内容
json_field() {
  local pat="\"$1\"[[:space:]]*:[[:space:]]*\"[^\"]*\""
  if [ -n "${2:-}" ]; then
    grep -o "$pat" "$2" 2>/dev/null | head -1
  else
    printf '%s' "$input" | grep -o "$pat" | head -1
  fi | sed 's/^[^:]*:[[:space:]]*"//; s/"$//'
}

sid="$(json_field session_id)"
transcript="$(json_field transcript_path)"
tool_use_id="$(json_field tool_use_id)"

# sid 会直接当文件名用：只保留安全字符，防路径穿越/非法文件名（UUID 原样通过）
sid="$(printf '%s' "$sid" | tr -cd 'A-Za-z0-9._-')"
[ -z "$sid" ] && exit 0

# transcript 含引号/反斜杠会写出非法 JSON（Rust 端解析失败即静默丢弃整个会话），直接弃用
case "$transcript" in
  *'"'*|*'\'*) transcript="" ;;
esac

file="$DIR/$sid.json"

# 旧 transcript 回退：部分事件的 payload 不含 transcript_path，无条件覆盖会让 TS 端丢掉
# mtime 兜底——权限批准后无法按转录推进解冻，只能干等 2h 超时
if [ -z "$transcript" ] && [ -f "$file" ]; then
  transcript="$(json_field transcript "$file")"
  case "$transcript" in
    *'"'*|*'\'*) transcript="" ;;
  esac
fi

# notify 的分类不需要旧状态，先判掉无关通知，免得为它们白抢一次锁
if [ "$arg" = "notify" ]; then
  case "$input" in
    *'"notification_type":"permission_prompt"'*) ;;
    *'"notification_type":"worker_permission_prompt"'*) ;;
    # 字段在、但不是权限类（idle_prompt 等）→ 明确忽略
    *'"notification_type"'*) exit 0 ;;
    # 字段整体缺失（anthropics/claude-code#11964）才退回文案兜底，否则会漏报权限提示
    *[Pp]ermission*) ;;
    *) exit 0 ;;
  esac
fi

# 串行化「读旧状态 → 判定 → 写新状态」：并行工具/子代理的 hook 会同时到达，
# 不加锁时后写者会覆盖先写者（例如 tool 覆盖刚落盘的 permission）。
# 锁提到事件分发之前，clean 的删除也落在同一临界区，不会与并发写入交错
lock="$DIR/.$sid.lock"
locked=0
i=0
while [ "$i" -lt 25 ]; do
  if mkdir "$lock" 2>/dev/null; then locked=1; break; fi
  i=$((i + 1))
  sleep 0.02
done
# 拿不到锁也照写：宁可短暂竞态，也不要卡住 hook（0.5s 后放弃等待）
cleanup() { [ "$locked" = 1 ] && rmdir "$lock" 2>/dev/null; }
trap cleanup EXIT

case "$arg" in
  clean)
    rm -f "$file"
    exit 0
    ;;
  start)
    # SessionStart：播种 idle。新会话立刻登记（带上 pid 便于死亡回收），
    # 同时覆盖 resume 时可能残留的冻结状态
    state="idle"
    ;;
  notify)
    state="permission"
    ;;
  denied)
    # 拒绝与批准一样是一次明确的「授权已处理」，模型随即继续干活 → 按工作中落盘
    state="thinking"
    ;;
  *)
    state="$arg"
    ;;
esac

# 授权请求类事件（PermissionRequest / Notification）不一定带 tool_use_id，缺失时沿用上一份状态
# 里的：同一个 tool_use_id 的 PostToolUse 只会出现在该工具真正放行执行之后，拿它当「授权已被
# 处理」的证据不会误判——工具没放行就永远等不到这条事件
if [ -z "$tool_use_id" ] && [ -f "$file" ]; then
  prev_state="$(json_field state "$file")"
  prev_ts="$(sed -n 's/.*"ts"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p' "$file" 2>/dev/null)"
  prev_tuid="$(json_field tool_use_id "$file")"
  # 紧邻的 PreToolUse（同一次工具调用）：限 5s 内，避免沿用陈旧 id 把并发的另一个待授权请求误判为已处理
  if [ "$prev_state" = "tool" ] && [ -n "$prev_ts" ] && [ $(( $(date +%s) - prev_ts )) -le 5 ]; then
    tool_use_id="$prev_tuid"
  # 同一笔待授权请求：PermissionRequest 落盘后，等了几秒的 Notification 会二次覆盖状态文件，
  # 不沿用就把待授权工具的 id 抹掉了，解冻只能退化成干等转录落盘
  elif [ "$prev_state" = "permission" ]; then
    tool_use_id="$prev_tuid"
  fi
fi
# 会写进 JSON：含引号/反斜杠即弃用，其余只留安全字符
tool_use_id="$(printf '%s' "$tool_use_id" | tr -cd 'A-Za-z0-9._-')"

# 权限待批准期间丢弃「工作态」写入：等待批准时并行工具/子代理仍会触发 PreToolUse/PostToolUse，
# 落盘的 tool/thinking 会覆盖尚未解除的 permission，状态在 alert/walking 间来回跳（黄灯闪跳）。
# 只拦 working 态——done/clean/denied 都是「授权已处理」的终局信号，不在闪烁链路上，放行可避免
# 权限异常时永久卡在黄灯。
# 两条解冻判据：
#   1) tool_use_id 命中——待授权工具自己的后续事件到了，说明授权已被处理（点批准）。转录落盘有
#      滞后（桌面端实测可达分钟级），这条不依赖它，是主判据
#   2) transcript 已越过权限时刻——授权后模型继续推进，转录必然有新增。2h 窗口与 TS 端
#      PERMISSION_TIMEOUT 对齐，避免陈旧状态永久挡住新写入
if [ "$arg" != "denied" ] && { [ "$state" = "thinking" ] || [ "$state" = "tool" ]; }; then
  if [ -f "$file" ]; then
    prev="$(cat "$file" 2>/dev/null || true)"
    case "$prev" in
      *'"state":"permission"'*)
        p_ts="$(printf '%s' "$prev" | sed -n 's/.*"ts"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p')"
        p_tr="$(json_field transcript "$file")"
        p_tuid="$(json_field tool_use_id "$file")"
        authorized=0
        if [ -n "$tool_use_id" ] && [ "$tool_use_id" = "$p_tuid" ]; then
          authorized=1
        fi
        if [ "$authorized" = 0 ] && [ -n "$p_ts" ] && [ -n "$p_tr" ] && [ -f "$p_tr" ]; then
          p_m="$(stat -f %m "$p_tr" 2>/dev/null || stat -c %Y "$p_tr" 2>/dev/null || echo 0)"
          if [ "$p_m" -le "$p_ts" ] && [ $(( $(date +%s) - p_ts )) -le 7200 ]; then
            exit 0
          fi
        fi
        ;;
    esac
  fi
fi

# 本 hook 由 Claude Code 直接 spawn（sh -c 的单条命令会被 exec 优化，无中间层），
# 所以 $PPID 就是该会话的 claude 进程；Rust 端据此 kill(pid,0) 回收死亡会话的状态文件
pid="${PPID:-0}"

tmp="$file.tmp.$$"
printf '{"state":"%s","ts":%s,"transcript":"%s","pid":%s,"tool_use_id":"%s"}\n' \
  "$state" "$(date +%s)" "$transcript" "$pid" "$tool_use_id" > "$tmp"
mv -f "$tmp" "$file"
