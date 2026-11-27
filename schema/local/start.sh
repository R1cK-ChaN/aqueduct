#!/usr/bin/env bash
# Start the container described by compose.yaml using the Docker CLI.
# Pass the password to Docker through the environment.
set -euo pipefail

if (( $# > 1 )); then
  echo "Usage: $0 [env-file]" >&2
  exit 2
fi
script_dir=$(dirname -- "$0")
env_file=${1:-$script_dir/.env}
if [[ ! -f "$env_file" ]]; then
  echo "Copy $script_dir/.env.example to $env_file and set CLICKHOUSE_PASSWORD first." >&2
  exit 1
fi
CDPATH= cd -- "$(dirname -- "$env_file")"
set -a
# shellcheck disable=SC1090
. "./${env_file##*/}"
set +a
: "${CLICKHOUSE_PASSWORD:?set CLICKHOUSE_PASSWORD in the env file}"

data_dir="${CLICKHOUSE_DATA_DIR:-$PWD/data}"
log_dir="${CLICKHOUSE_LOG_DIR:-$PWD/logs}"
mkdir -p "$data_dir" "$log_dir"

docker run -d --name hlmm-clickhouse --restart unless-stopped \
  -p "127.0.0.1:${CLICKHOUSE_HTTP_PORT:-8123}:8123" \
  -p "127.0.0.1:${CLICKHOUSE_NATIVE_PORT:-9000}:9000" \
  -e CLICKHOUSE_USER=hlmm -e CLICKHOUSE_PASSWORD -e CLICKHOUSE_DEFAULT_ACCESS_MANAGEMENT=1 \
  -v "$data_dir:/var/lib/clickhouse" -v "$log_dir:/var/log/clickhouse-server" \
  --ulimit nofile=262144:262144 \
  clickhouse/clickhouse-server:26.8
