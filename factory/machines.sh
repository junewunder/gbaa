# factory/machines.sh: sourced. Brings in the wfactory skill's runtime/machines.sh
# (machine_rows, machine_get: docs/machines.md's table), the copy factory/skill picks.
. "$(dirname "$(realpath "${BASH_SOURCE[0]}")")/skill" && . "$FACTORY_SKILL/runtime/machines.sh"
