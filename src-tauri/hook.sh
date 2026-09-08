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

case "$1" in
  clean)
    rm -f "$DIR/$sid.json"
    exit 0
    ;;
  notify)
    # 仅权限类通知写入状态；其余通知忽略
    case "$input" in
      *permission*|*approve*|*allow*) ;;
      *) exit 0 ;;
    esac
    state="permission"
    ;;
  *)
    state="$1"
    ;;
esac

tmp="$DIR/$sid.json.tmp.$$"
printf '{"state":"%s","ts":%s,"transcript":"%s"}\n' \
  "$state" "$(date +%s)" "$transcript" > "$tmp"
mv -f "$tmp" "$DIR/$sid.json"
