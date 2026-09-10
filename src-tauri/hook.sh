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

arg="${1:-}"
case "$arg" in
  thinking|tool|permission|done|notify|clean) ;;
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

case "$arg" in
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
    state="$arg"
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
        p_tr="$(json_field transcript "$file")"
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
