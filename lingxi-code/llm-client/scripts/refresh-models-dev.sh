#!/usr/bin/env bash
set -euo pipefail

# Refresh only the provider slices shipped by LingXi. The ChatGPT-login slice
# is hand-authored because that backend exposes its catalog dynamically.
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
crate_dir="$(cd "$script_dir/.." && pwd)"
data_dir="$crate_dir/data/models-dev"
source_url="${MODELS_DEV_URL:-https://models.dev/api.json}"
snapshot="$(mktemp)"
trap 'rm -f "$snapshot"' EXIT

curl --fail --silent --show-error --location "$source_url" --output "$snapshot"

while IFS=$'\t' read -r provider_id output_name; do
  jq --arg id "$provider_id" -e '.[$id]' "$snapshot" > "$data_dir/$output_name.json"
done <<'EOF'
openrouter	openrouter
deepseek	deepseek
moonshotai-cn	kimi
kimi-for-coding	kimi-code
zhipuai-coding-plan	zhipuai-coding-plan
zai	zai
openai	openai
github-copilot	github-copilot
google	gemini
EOF

for file in "$data_dir"/*.json; do
  jq -e '.id and .name and (.models | type == "object")' "$file" >/dev/null
done

# Keep the refresh reproducible and fail loudly when upstream removes, renames,
# or deprecates a model that LingXi exposes in its curated pickers. Anthropic is
# intentionally absent: its first-party table is maintained in Rust, while the
# ChatGPT-account slice is hand-authored and validated below.
while IFS=$'\t' read -r slice_name model_id; do
  jq --arg model "$model_id" -e '
    .models[$model]
    and ((.models[$model].status // "") != "deprecated")
  ' "$data_dir/$slice_name.json" >/dev/null || {
    echo "Curated model missing or deprecated: $slice_name/$model_id" >&2
    exit 1
  }
done <<'EOF'
openrouter	openrouter/auto
openrouter	openrouter/free
openrouter	openrouter/bodybuilder
openrouter	openrouter/healer-alpha
openrouter	openrouter/hunter-alpha
openrouter	openrouter/pony-alpha
deepseek	deepseek-v4-flash
deepseek	deepseek-v4-flash-vision-exp
deepseek	deepseek-v4-pro
kimi	kimi-k3
kimi-code	k3
zhipuai-coding-plan	glm-5.3
zai	glm-5.3
openai	gpt-5.6-sol
openai	gpt-5.6-terra
openai	gpt-5.6-luna
github-copilot	claude-opus-5
github-copilot	claude-sonnet-5
github-copilot	gemini-3.7-flash
github-copilot	gpt-5.6-sol
github-copilot	gpt-5.6-terra
github-copilot	gpt-5.6-luna
gemini	gemini-3.7-flash
gemini	gemini-3.6-flash
gemini	gemini-3.5-flash
gemini	gemini-3.1-pro-preview
EOF

for model_id in gpt-5.6-sol gpt-5.6-terra gpt-5.6-luna; do
  jq --arg model "$model_id" -e '.models[$model]' \
    "$data_dir/openai-chatgpt.json" >/dev/null || {
    echo "Hand-authored ChatGPT model missing: $model_id" >&2
    exit 1
  }
done

echo "Refreshed models.dev provider slices in $data_dir"
