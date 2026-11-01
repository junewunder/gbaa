extern crate std;
use super::GameName;
use std::string::{String, ToString};

fn name(s: &str) -> String {
    GameName(s).to_string()
}

/// Runs of other characters are one dash, none at either end, as the
/// justfile's `tr`/`sed` names `out/<name>/`.
#[test]
fn names_as_the_justfile_does() {
    assert_eq!(
        name("Kirby & The Amazing Mirror"),
        "kirby-the-amazing-mirror"
    );
    assert_eq!(
        name("WarioWare, Inc. - Mega Microgame$!"),
        "warioware-inc-mega-microgame"
    );
    assert_eq!(
        name("Pokemon - Emerald Version (U)"),
        "pokemon-emerald-version-u"
    );
    assert_eq!(name("--already-a-name--"), "already-a-name");
    // Not ASCII is another character, as `[^a-z0-9]` has it.
    assert_eq!(name("Pokémon Émeraude"), "pok-mon-meraude");
    // With none, a name all the same.
    assert_eq!(name("$$$"), "game");
    assert_eq!(name(""), "game");
}
