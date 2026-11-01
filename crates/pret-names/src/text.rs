//! pret's character table, `charmap.txt` at a decomp's root, read as data
//! into the source map's `[text]` (`codegen::text` decodes with it): each
//! entry's bytes and its quoted character or its name, in the file's
//! order. Comments (`@ …`) and blank lines are dropped; nothing else of the
//! file is kept.

use codegen::text::Entry;
use std::fmt::Write;

/// The entries of a `charmap.txt`: `'A' = BB`, `PLAYER = FD 01`
/// ([`codegen::text::parse_charmap`], one-byte units). A line that is
/// neither is an error, with its number.
pub fn parse(text: &str) -> Result<Vec<(Vec<u8>, Entry)>, String> {
    codegen::text::parse_charmap(text, 1)
}

/// The source map's `[text]`: `file`, and each entry as `{ bytes = "FC 06
/// 00", name = "FONT_SMALL" }` or `{ bytes = "BB", char = "A" }`.
pub fn toml(entries: &[(Vec<u8>, Entry)]) -> String {
    let mut o = String::from(
        "\n# pret's character table (charmap.txt), as data: each entry's bytes and its\n\
         # character or name, in the file's order. codegen::text decodes with it.\n\
         [text]\nfile = \"charmap.txt\"\nchars = [\n",
    );
    for (bytes, e) in entries {
        let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02X}")).collect();
        let (k, v) = match e {
            Entry::Char(c) => ("char", c),
            Entry::Name(n) => ("name", n),
        };
        writeln!(
            o,
            "  {{ bytes = \"{}\", {k} = {} }},",
            hex.join(" "),
            crate::quoted(v)
        )
        .unwrap();
    }
    o.push_str("]\n");
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_charmap_entries_and_writes_them_as_the_map_loads_them() {
        let src = "' '         = 00\n\
                   '\\''        = B4\n\
                   PKMN        = 53 54\n\
                   \n\
                   @ string placeholders\n\
                   PLAYER      = FD 01 @ the player's name\n\
                   '\\n' = FE @ new line\n";
        let e = parse(src).unwrap();
        assert_eq!(
            e,
            vec![
                (vec![0x00], Entry::Char(" ".into())),
                (vec![0xb4], Entry::Char("'".into())),
                (vec![0x53, 0x54], Entry::Name("PKMN".into())),
                (vec![0xfd, 0x01], Entry::Name("PLAYER".into())),
                (vec![0xfe], Entry::Char("\\n".into())),
            ]
        );
        let doc: toml::Table = toml(&e).parse().unwrap();
        let back = codegen::text::parse_entries(&toml::Value::Table(
            doc["text"].as_table().unwrap().clone(),
        ));
        assert_eq!(back.unwrap(), e);
        assert!(parse("FOO\n").is_err());
        assert!(parse("'A' = ZZ\n").is_err());
    }
}
