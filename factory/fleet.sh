#!/usr/bin/env bash
# factory/fleet.sh: runs the wfactory skill's runtime/fleet.sh, the copy factory/skill
# picks. Don't edit: the code lives in the skill (`factory/fleet.sh --help`).
. "$(dirname "$(realpath "$0")")/skill" && exec "$FACTORY_SKILL/runtime/fleet.sh" "$@"
