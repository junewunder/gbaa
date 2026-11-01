#!/usr/bin/env bash
# factory/long.sh: runs the wfactory skill's runtime/long.sh, the copy factory/skill
# picks. Don't edit: the code lives in the skill (`factory/long.sh --help`).
. "$(dirname "$(realpath "$0")")/skill" && exec "$FACTORY_SKILL/runtime/long.sh" "$@"
