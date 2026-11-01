#!/usr/bin/env bash
# factory/lane.sh: runs the wfactory skill's runtime/lane.sh, the copy factory/skill
# picks. Don't edit: the code lives in the skill (`factory/lane.sh --help`).
. "$(dirname "$(realpath "$0")")/skill" && exec "$FACTORY_SKILL/runtime/lane.sh" "$@"
