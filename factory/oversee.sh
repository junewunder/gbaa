#!/usr/bin/env bash
# factory/oversee.sh: runs the wfactory skill's runtime/oversee.sh, the copy factory/skill
# picks. Don't edit: the code lives in the skill (`factory/oversee.sh --help`).
. "$(dirname "$(realpath "$0")")/skill" && exec "$FACTORY_SKILL/runtime/oversee.sh" "$@"
