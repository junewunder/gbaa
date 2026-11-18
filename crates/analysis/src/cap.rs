//! The memory-cap guard of the heavy tools (`gbaa-lift`, `gbaa-recompile`,
//! `gbaa-holes`, `gbaa-lift-check`, `route-check`): [`require_cap`], first
//! thing in their `main`, refuses to run unless the process is under a
//! memory cap of its own (`docs/factory.md`, "Resources"), so a runaway
//! dies in its own scope instead of taking the desktop down.
//!
//! A cap is a cgroup v2 `memory.max` other than `max`, on the process's
//! cgroup or any ancestor. A cap that is also over the session leader (the
//! shell the command was typed in) doesn't count: that's the agent's or the
//! terminal's own cgroup (a lane's session scope, an app slice), which a
//! bare run shares with everything else there. Inside a factory capped job
//! (`just capped`, `just long`, which set `FACTORY_CAPPED_IN`) the leader
//! is the job's own, so there any cap counts. `GBAA_UNCAPPED=1` skips the
//! check; no cgroup v2 to read (another OS, cgroup v1) allows the run.

/// What the cgroups say about the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cap {
    /// Under a cap of its own.
    Capped,
    /// No cap, or only ones it shares with its session leader.
    Uncapped,
    /// No `memory.max` could be read: can't tell.
    Unknown,
}

/// The cgroup v2 path in `/proc/<pid>/cgroup`'s text: the `0::/path` line.
pub fn cgroup_path(proc_cgroup: &str) -> Option<&str> {
    proc_cgroup
        .lines()
        .find_map(|l| l.strip_prefix("0::"))
        .map(str::trim)
        .filter(|p| p.starts_with('/'))
}

/// The session id in `/proc/<pid>/stat`'s text (its sixth field, after the
/// parenthesised name, which may hold spaces). `None` for 0 or bad text.
pub fn session_of(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace()
        .nth(3)?
        .parse()
        .ok()
        .filter(|&s| s != 0)
}

/// Whether cgroup `outer` is `inner` or one of its ancestors.
fn covers(outer: &str, inner: &str) -> bool {
    outer == inner
        || inner
            .strip_prefix(outer)
            .is_some_and(|r| r.starts_with('/'))
}

/// The verdict for a process in cgroup `own` whose session leader is in
/// `leader` (`None`: unknown), `in_job` if inside a factory capped job.
/// `memory_max` reads a cgroup's `memory.max` (`None`: no such file).
pub fn verdict(
    own: &str,
    leader: Option<&str>,
    in_job: bool,
    memory_max: impl Fn(&str) -> Option<String>,
) -> Cap {
    let mut seen = false;
    let mut at = own.trim_end_matches('/');
    while !at.is_empty() {
        if let Some(v) = memory_max(at) {
            seen = true;
            let own_cap = in_job || leader.is_none_or(|l| !covers(at, l));
            if v.trim() != "max" && own_cap {
                return Cap::Capped;
            }
        }
        at = &at[..at.rfind('/').unwrap_or(0)];
    }
    if seen { Cap::Uncapped } else { Cap::Unknown }
}

/// The refusal `tool` prints.
pub fn refusal(tool: &str) -> String {
    format!(
        "{tool}: not under a memory cap. Run it through `just capped`, `just long start`, or a recipe that caps it.\n\
         (GBAA_UNCAPPED=1 runs it anyway, for a small input you've measured.)"
    )
}

/// Exit 2 with [`refusal`] unless this process is under a memory cap of
/// its own (the module's doc says what counts), or `GBAA_UNCAPPED=1`.
pub fn require_cap(tool: &str) {
    if std::env::var("GBAA_UNCAPPED").is_ok_and(|v| v == "1") {
        return;
    }
    if here() == Cap::Uncapped {
        eprintln!("{}", refusal(tool));
        std::process::exit(2);
    }
}

/// This process's verdict, from `/proc` and `/sys/fs/cgroup`.
fn here() -> Cap {
    let read = |p: &str| std::fs::read_to_string(p).ok();
    let Some(text) = read("/proc/self/cgroup") else {
        return Cap::Unknown;
    };
    let Some(own) = cgroup_path(&text) else {
        return Cap::Unknown;
    };
    let leader = read("/proc/self/stat")
        .as_deref()
        .and_then(session_of)
        .and_then(|s| read(&format!("/proc/{s}/cgroup")));
    let leader = leader.as_deref().and_then(cgroup_path);
    let in_job = std::env::var_os("FACTORY_CAPPED_IN").is_some_and(|v| !v.is_empty());
    verdict(own, leader, in_job, |cg| {
        read(&format!("/sys/fs/cgroup{cg}/memory.max"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const APP: &str = "/user.slice/user-1000.slice/user@1000.service/app.slice";

    fn maxes(caps: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let m: HashMap<String, String> = caps
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |p| m.get(p).cloned()
    }

    #[test]
    fn parses_proc_files() {
        assert_eq!(
            cgroup_path("0::/user.slice/a.scope\n"),
            Some("/user.slice/a.scope")
        );
        // A hybrid system's v1 lines come first.
        assert_eq!(cgroup_path("1:name=systemd:/x\n0::/y\n"), Some("/y"));
        assert_eq!(cgroup_path("12:memory:/x\n"), None);
        assert_eq!(
            session_of("4242 (my tool) S 4200 4242 4100 34816 4242 4194304"),
            Some(4100)
        );
        assert_eq!(session_of("1 (systemd) S 0 1 0 0"), None);
        assert_eq!(session_of("garbage"), None);
    }

    #[test]
    fn a_scope_of_its_own_counts() {
        let job = format!("{APP}/run-r1.scope");
        let shell = format!("{APP}/konsole.scope");
        let m = maxes(&[(&*job, "3221225472\n"), (&*shell, "max\n"), (APP, "max\n")]);
        assert_eq!(verdict(&job, Some(&*shell), false, &m), Cap::Capped);
        // Its own leader, though, makes the same scope a shell's.
        assert_eq!(verdict(&job, Some(&*job), false, &m), Cap::Uncapped);
    }

    #[test]
    fn an_ancestor_cap_counts() {
        let slice = format!("{APP}/job.slice");
        let job = format!("{slice}/run-r1.scope");
        let shell = format!("{APP}/konsole.scope");
        let m = maxes(&[(&*job, "max"), (&*slice, "8589934592"), (APP, "max")]);
        assert_eq!(verdict(&job, Some(&*shell), false, &m), Cap::Capped);
    }

    #[test]
    fn a_cap_shared_with_the_session_doesnt() {
        // A bare run in an agent's capped session scope, under a capped slice.
        let slice = format!("{APP}/agents.slice");
        let session = format!("{slice}/agent-1.scope");
        let m = maxes(&[
            (&*session, "10737418240"),
            (&*slice, "28586217472"),
            (APP, "max"),
        ]);
        assert_eq!(verdict(&session, Some(&*session), false, &m), Cap::Uncapped);
        // A sibling of the session's slice isn't over it: `agents.slice2`.
        let other = format!("{APP}/agents.slice2/x.scope");
        let m2 = maxes(&[(&*other, "max"), (&*format!("{APP}/agents.slice2"), "1G")]);
        assert_eq!(verdict(&other, Some(&*session), false, &m2), Cap::Capped);
        // Inside a factory capped job, the job's own cap counts.
        let svc = format!("{APP}/long-x.service");
        let m3 = maxes(&[(&*svc, "8589934592"), (APP, "max")]);
        assert_eq!(verdict(&svc, Some(&*svc), true, &m3), Cap::Capped);
        assert_eq!(
            verdict(&svc, Some(&*svc), true, maxes(&[(&*svc, "max")])),
            Cap::Uncapped
        );
    }

    #[test]
    fn nothing_to_read_is_unknown() {
        assert_eq!(verdict("/a/b", None, false, maxes(&[])), Cap::Unknown);
        assert_eq!(
            verdict("/a/b", None, false, maxes(&[("/a", "max")])),
            Cap::Uncapped
        );
        assert_eq!(
            verdict("/a/b", None, false, maxes(&[("/a", "100")])),
            Cap::Capped
        );
    }

    #[test]
    fn refusal_names_the_tool() {
        assert!(refusal("gbaa-lift").starts_with("gbaa-lift: not under a memory cap."));
    }
}
