#!/usr/bin/env bash
# renovate --platform=local(dryRun=lookup)の唯一の入口。
#
# 目的: push を待たずローカルで全エコシステム(cargo / dockerfile / github-actions /
#       mise / devcontainer)横断の「依存の古さ」を確認する read-only な lookup。
#       PR ボットは Dependabot(.github/dependabot.yml)。remote には一切書かない。
#       注意: Renovate local は **git 追跡済み**(commit 済み or `git add` 済み)の
#       マニフェストだけを見る。pre-push フックなら当然追跡済みなので自動で機能する。
#
# 出力: RENOVATE_REPORT_TYPE=logging で更新候補(depName / newVersion / updateType /
#       currentVersion)と outdated 件数・libYears を INFO レベルで人間可読に出力する。
#
# 実行基盤: 公式 Docker イメージ renovate/renovate。イメージが Renovate のサポート
#       Node を同梱するためホスト Node に非依存(ホスト Node が新しすぎて弾かれる問題を
#       構造的に回避)。`:latest` は同梱 Node が常に整合するので安全。再現性が要るなら
#       RENOVATE_IMAGE で digest pin を渡す。
#
#       Docker は「雑に root 実行しない」— aozora の compose と同じ非 root / root 所有物を
#       残さない方針を docker run フラグで踏襲する:
#         --user $(id -u):$(id -g)            ホストユーザで実行。root 化せず、万一の
#                                             書込みも host 所有(root 所有のゴミを残さない)。
#                                             uid が bind mount の所有者と一致するので git の
#                                             dubious-ownership も起きず safe.directory 不要。
#         --tmpfs /tmp + HOME / BASE_DIR      HOME と Renovate キャッシュを tmpfs へ。任意
#                                             uid で書込み可、ホストにも repo にも痕跡なし。
#         --cap-drop ALL / no-new-privileges  能力剥奪 + 権限昇格禁止(非 root をさらに締める)。
#         --init                              PID1 シグナル処理(ゾンビ回収)。
#       ソース mount は読むだけ。dryRun=lookup なので書込みは発生しない。
#
# トークン: 未認証だと GitHub データソースが 60 req/h に制限され lookup が詰まる。
#       解決順 GITHUB_COM_TOKEN → GITHUB_TOKEN → `gh auth token`。どれも無ければ起動せず
#       exit 1(未認証で走る経路を作らない)。新規 PAT は不要、gh セッションを再利用。
set -euo pipefail

command -v docker >/dev/null 2>&1 \
  || { echo "error: docker が必要です。" >&2; exit 1; }

token="${GITHUB_COM_TOKEN:-${GITHUB_TOKEN:-$(gh auth token 2>/dev/null || true)}}"
[[ -n "${token}" ]] \
  || { echo "error: GitHub 未認証。'gh auth login' を実行(未認証だと API は 60 req/h)。" >&2; exit 1; }

repo_root="$(git rev-parse --show-toplevel)"
image="${RENOVATE_IMAGE:-renovate/renovate:latest}"

exec docker run --rm --init \
  --user "$(id -u):$(id -g)" \
  --cap-drop ALL \
  --security-opt no-new-privileges \
  --tmpfs /tmp:rw,exec,nosuid,nodev \
  --env HOME=/tmp \
  --env RENOVATE_BASE_DIR=/tmp/renovate \
  --env RENOVATE_PLATFORM=local \
  --env RENOVATE_ONBOARDING=false \
  --env RENOVATE_REQUIRE_CONFIG=optional \
  --env RENOVATE_REPORT_TYPE=logging \
  --env RENOVATE_CONFIG_FILE=/repo/renovate.local.json5 \
  --env GITHUB_COM_TOKEN="${token}" \
  --volume "${repo_root}:/repo" \
  --workdir /repo \
  "${image}" "$@"
