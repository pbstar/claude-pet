#!/bin/bash
# claude-pet 状态 hook（极简，纯 shell，不依赖 node）
# 事件参数由安装器在 settings.json 里指定；Claude 的 hook JSON 从 stdin 传入。
# usage: hook.sh <thinking|tool|permission|done|notify|clean>
#   - thinking/tool/permission/done：写会话状态文件
#   - notify：仅当通知是权限提示时才写 permission（过滤 idle_prompt 等无关通知）
#   - clean：删除会话状态（对应 SessionEnd）
set -u
DIR="$HOME/.claude/claude-pet"
mkdir -p "$DIR"

input="$(cat 2>/dev/null || true)"
sid="$(printf '%s' "$input" | sed -n 's/.*"session_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
transcript="$(printf '%s' "$input" | sed -n 's/.*"transcript_path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"

# 无 session_id 时无从落盘，直接返回
[ -z "$sid" ] && exit 0

file="$DIR/$sid.json"

case "$1" in
  clean)
    rm -f "$file"
    rmdir "$DIR/.$sid.lock" 2>/dev/null
    exit 0
    ;;
  notify)
    # 按结构化的 notification_type 判定，而非全文匹配（message 文案会变，且可能含 allow 等词）
    case "$input" in
      *'"notification_type":"permission_prompt"'*) ;;
      *'"notification_type":"worker_permission_prompt"'*) ;;
      *) exit 0 ;;
    esac
    state="permission"
    ;;
  *)
    state="$1"
    ;;
esac

# 串行化「读旧状态 → 判定 → 写新状态」：并行工具/子代理的 hook 会同时到达，
# 不加锁时后写者会覆盖先写者（例如 tool 覆盖刚落盘的 permission）
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

# 权限待批准期间丢弃「工作态」写入：等待批准时并行工具/子代理仍会触发 PreToolUse/PostToolUse，
# 落盘的 tool/thinking 会覆盖尚未解除的 permission，状态在 alert/walking 间来回跳（黄灯闪跳）。
# 只拦 working 态——done/clean 不在闪烁链路上，放行可避免权限异常时永久卡在黄灯。
# 判据与 TS 端解冻逻辑一致：transcript 未推进（mtime <= 权限 ts）即权限仍未解除。
if [ "$state" = "thinking" ] || [ "$state" = "tool" ]; then
  if [ -f "$file" ]; then
    prev="$(cat "$file" 2>/dev/null || true)"
    case "$prev" in
      *'"state":"permission"'*)
        p_ts="$(printf '%s' "$prev" | sed -n 's/.*"ts"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p')"
        p_tr="$(printf '%s' "$prev" | sed -n 's/.*"transcript"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
        if [ -n "$p_ts" ] && [ -n "$p_tr" ] && [ -f "$p_tr" ]; then
          p_m="$(stat -f %m "$p_tr" 2>/dev/null || stat -c %Y "$p_tr" 2>/dev/null || echo 0)"
          # 2h 窗口与 TS 端 PERMISSION_TIMEOUT 对齐，避免陈旧状态永久挡住新写入
          if [ "$p_m" -le "$p_ts" ] && [ $(( $(date +%s) - p_ts )) -le 7200 ]; then
            exit 0
          fi
        fi
        ;;
    esac
  fi
fi

tmp="$file.tmp.$$"
printf '{"state":"%s","ts":%s,"transcript":"%s"}\n' \
  "$state" "$(date +%s)" "$transcript" > "$tmp"
mv -f "$tmp" "$file"
