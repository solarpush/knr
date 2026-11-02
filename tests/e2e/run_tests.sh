#!/usr/bin/env bash
set -euo pipefail

# knr E2E Test Suite Runner
# Usage: ./tests/e2e/run_tests.sh [--all | --tier <1|2|3|4> | --wave <1|2|3>]

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MANIFEST_PATH="${SCRIPT_DIR}/Cargo.toml"

TIER=""
WAVE=""
RUN_ALL=true

while [[ $# -gt 0 ]]; do
    case "$1" in
        --all)
            RUN_ALL=true
            shift
            ;;
        --tier)
            TIER="$2"
            RUN_ALL=false
            shift 2
            ;;
        --wave)
            WAVE="$2"
            RUN_ALL=false
            shift 2
            ;;
        --help|-h)
            echo "Usage: $0 [--all] [--tier <1|2|3|4>] [--wave <1|2|3>]"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--all] [--tier <1|2|3|4>] [--wave <1|2|3>]"
            exit 3
            ;;
    esac
done

echo "=========================================================="
echo " Running knr E2E Test Suite"
echo " Manifest: ${MANIFEST_PATH}"
echo "=========================================================="

FILTER=""
if [[ -n "${TIER}" ]]; then
    FILTER="tier${TIER}"
    echo "Filtering by Tier: ${TIER}"
elif [[ -n "${WAVE}" ]]; then
    FILTER="wave${WAVE}"
    echo "Filtering by Wave: ${WAVE}"
else
    echo "Running all tests"
fi

if [[ -n "${FILTER}" ]]; then
    cargo test --offline --manifest-path "${MANIFEST_PATH}" --lib "${FILTER}"
else
    cargo test --offline --manifest-path "${MANIFEST_PATH}"
fi

EXIT_CODE=$?
echo "=========================================================="
if [[ ${EXIT_CODE} -eq 0 ]]; then
    echo " [PASSED] E2E Test Suite completed successfully."
else
    echo " [FAILED] E2E Test Suite failed with exit code: ${EXIT_CODE}"
fi
echo "=========================================================="
exit ${EXIT_CODE}
