//! A game's name, the key its saves, states and resume file are filed
//! under (docs/save-locations-design.md): lower case, each run of other
//! characters one `-`, none at either end, so it is a safe folder name on
//! any OS. "Kirby & The Amazing Mirror" is `kirby-the-amazing-mirror`, as
//! `just recompile` names `out/<name>/`.

use core::fmt;

/// [`fmt::Display`]s `.0` as a game's name; `game` when `.0` has no
/// ASCII letter or digit. The one rule for it: the recompiler bakes it
/// into a build, and the desktop front end takes it from a ROM's file
/// name.
#[derive(Clone, Copy, Debug)]
pub struct GameName<'a>(pub &'a str);

impl fmt::Display for GameName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use fmt::Write;
        let mut dash = false;
        let mut any = false;
        for c in self.0.chars() {
            if c.is_ascii_alphanumeric() {
                if dash && any {
                    f.write_char('-')?;
                }
                f.write_char(c.to_ascii_lowercase())?;
                dash = false;
                any = true;
            } else {
                dash = true;
            }
        }
        if !any {
            f.write_str("game")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
