//! The lift check's programs (`crates/codegen/lift-check.toml`; unit
//! `native/lift-bars`): one table per program, read by `gbaa-lift-check`
//! (the `just lift-check` driver) and, for its bars, by `gbaa-lift
//! --bars` ([`super::metrics::bars`]). A unit adds a program by adding a
//! table.

/// The default bars file, from the repo root.
pub const FILE: &str = "crates/codegen/lift-check.toml";

/// One program of the lift check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    /// The table's key: the ROM's file stem, lower case (what `gbaa-lift`
    /// names it, and finds its bars and `holes/<name>.toml` by).
    pub name: String,
    /// The ROM, under `$GBAA_DATA`.
    pub rom: String,
    /// The route `route-check --native` runs it on, from the repo root.
    pub route: String,
    /// More `gbaa-lift` flags.
    pub lift_flags: Vec<String>,
    /// More `route-check` flags.
    pub route_flags: Vec<String>,
    /// The planted wrong store (`gbaa-lift --plant AT`) the native check
    /// must catch (`route-check --planted`).
    pub planted_store: Option<u32>,
    /// The planted hard hole: the stand-in dropped (`gbaa-lift
    /// --drop-standin KEY`), and the error its crate's build must fail with.
    pub planted_hole: Option<(u32, String)>,
    /// `gbaa-lift` flags that each bring an old behaviour back (one lift
    /// per flag); the native check must catch every one (`route-check
    /// --planted`).
    pub planted_flags: Vec<String>,
}

/// The programs of a lift-check file, in its order. Each must have a bars
/// table (checked by `gbaa-lift --bars`, not here).
pub fn programs(text: &str) -> Result<Vec<Program>, String> {
    let t: toml::Table = text.parse().map_err(|e| format!("{e}"))?;
    let ps = t
        .get("program")
        .and_then(|p| p.as_table())
        .ok_or("no [program.<name>] tables")?;
    let mut out = Vec::new();
    for (name, p) in ps {
        let p = p.as_table().ok_or(format!("program.{name}: not a table"))?;
        let at = |k: &str| format!("program.{name}.{k}");
        let s = |k: &str| -> Result<String, String> {
            p.get(k)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or(format!("{}: missing, or not a string", at(k)))
        };
        let list = |k: &str| -> Result<Vec<String>, String> {
            match p.get(k) {
                None => Ok(Vec::new()),
                Some(v) => v
                    .as_array()
                    .and_then(|a| a.iter().map(|x| x.as_str().map(str::to_string)).collect())
                    .ok_or(format!("{}: not a list of strings", at(k))),
            }
        };
        let addr = |v: &toml::Value, k: &str| -> Result<u32, String> {
            v.as_integer()
                .and_then(|i| u32::try_from(i).ok())
                .ok_or(format!("{}: not an address", at(k)))
        };
        for k in p.keys() {
            if ![
                "rom",
                "route",
                "lift_flags",
                "route_flags",
                "planted_store",
                "planted_hole",
                "planted_flags",
                "bars",
            ]
            .contains(&k.as_str())
            {
                return Err(format!("{}: unknown key", at(k)));
            }
        }
        let planted_hole = match p.get("planted_hole") {
            None => None,
            Some(h) => {
                let h = h
                    .as_table()
                    .ok_or(format!("{}: not a table", at("planted_hole")))?;
                let key = addr(
                    h.get("drop_standin")
                        .ok_or(format!("{}: missing", at("planted_hole.drop_standin")))?,
                    "planted_hole.drop_standin",
                )?;
                let names = h
                    .get("names")
                    .and_then(|v| v.as_str())
                    .ok_or(format!("{}: missing", at("planted_hole.names")))?;
                Some((key, names.to_string()))
            }
        };
        // The bars and the built crates go by the table's key.
        if list("lift_flags")?.iter().any(|f| f == "--name") {
            return Err(format!(
                "{}: --name: the table's key is the name",
                at("lift_flags")
            ));
        }
        if !p.get("bars").is_some_and(|b| b.is_table()) {
            return Err(format!("{}: missing", at("bars")));
        }
        out.push(Program {
            name: name.clone(),
            rom: s("rom")?,
            route: s("route")?,
            lift_flags: list("lift_flags")?,
            route_flags: list("route_flags")?,
            planted_store: p
                .get("planted_store")
                .map(|v| addr(v, "planted_store"))
                .transpose()?,
            planted_hole,
            planted_flags: list("planted_flags")?,
        });
    }
    if out.is_empty() {
        return Err("no programs".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn the_committed_file_reads_and_its_routes_exist() {
        let text = std::fs::read_to_string(root().join(FILE)).unwrap();
        let ps = programs(&text).unwrap();
        assert!(ps.len() >= 3);
        for p in &ps {
            assert!(root().join(&p.route).exists(), "{}: {}", p.name, p.route);
            assert_eq!(
                std::path::Path::new(&p.rom)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_lowercase()),
                Some(p.name.clone()),
                "the table's key is the ROM's name"
            );
            assert!(super::super::metrics::bars(&text, &p.name).is_ok());
        }
        assert!(ps.iter().any(|p| p.planted_store.is_some()));
        assert!(ps.iter().any(|p| p.planted_hole.is_some()));
        // native/lift-fallthrough's control.
        assert!(
            ps.iter()
                .any(|p| p.planted_flags.iter().any(|f| f == "--fall-as-tail-call"))
        );
        // native/lift-irq-hold's.
        assert!(
            ps.iter()
                .any(|p| p.planted_flags.iter().any(|f| f == "--irq-at-waits"))
        );
    }

    #[test]
    fn bad_tables_fail() {
        let ok = "[program.a]\nrom = \"a.gba\"\nroute = \"r\"\n[program.a.bars]\n";
        assert_eq!(programs(ok).unwrap()[0].name, "a");
        assert!(programs("[program.a]\nrom = \"a.gba\"\nroute = \"r\"\n").is_err());
        assert!(programs(&format!("{ok}\n[program.b]\nrom = \"b.gba\"\n")).is_err());
        let named = ok.replace(
            "[program.a.bars]",
            "lift_flags = [\"--name\", \"b\"]\n[program.a.bars]",
        );
        assert!(programs(&named).is_err());
        assert!(
            programs(
                "[program.a]\nrom = \"a.gba\"\nroute = \"r\"\nplanted = 1\n[program.a.bars]\n"
            )
            .is_err()
        );
        let flags = ok.replace(
            "[program.a.bars]",
            "planted_flags = [\"--x\"]\n[program.a.bars]",
        );
        assert_eq!(programs(&flags).unwrap()[0].planted_flags, vec!["--x"]);
        let bad = ok.replace("[program.a.bars]", "planted_flags = 1\n[program.a.bars]");
        assert!(programs(&bad).is_err());
    }
}
