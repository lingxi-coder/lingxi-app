#!/usr/bin/env bash
# i18n 门 —— 驱动（规则在 scripts/checks/check_i18n_pairing.py）。
#
# ⛔ 永远不要用 `generate.py --check --ios-out X --android-out Y` 当判据：
# 那条路径一个字节都不比对，输出与真检查逐字节相同，退出码都是 0。
# 真检查是不带 out 参数的那个：
#
#     python3 clients/translations/generate.py --check
#
# 本脚本是它的补充,不是替代:它管的是 key 齐了之后仍然会静默丢失的三类东西。
set -euo pipefail
cd "$(dirname "$0")/../.."

ENGINE="scripts/checks/check_i18n_pairing.py"
if [[ ! -f "$ENGINE" ]]; then
    echo "check-i18n-pairing: engine missing at $ENGINE" >&2
    exit 1
fi

exec python3 "$ENGINE" "$@"
