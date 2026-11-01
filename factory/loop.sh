#!/usr/bin/env bash
# factory/loop.sh: runs the wfactory skill's runtime/loop.sh, the copy factory/skill
# picks. Don't edit: the code lives in the skill (`factory/loop.sh --help`).
. "$(dirname "$(realpath "$0")")/skill" && exec "$FACTORY_SKILL/runtime/loop.sh" "$@"
