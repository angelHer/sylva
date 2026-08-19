//! Resolving and launching a terminal for a worktree.
//!
//! `plan` is the whole decision tree — multiplexer, then emulator, then
//! argv shape — kept as one pure function over an [`Environment`] so the
//! ordering rules can be tested without a real `PATH` or a real terminal.
//! `SystemEnvironment` is the only thing here that reads `std::env`; nothing
//! else in this module, and nothing in `application`, is allowed to.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;

use crate::application::{Launch, TerminalError, TerminalLauncher, TerminalRequest};

/// Everything `plan` needs to know about the machine it is running on,
/// behind a trait so the decision tree can be driven by a fake in tests.
pub trait Environment {
    /// Looks a program up by name, the way a shell's `PATH` search would,
    /// returning the resolved path if it exists and looks runnable.
    fn find_program(&self, name: &str) -> Option<PathBuf>;

    /// `$TERMINAL`, split on ASCII whitespace with no shell quoting. Empty
    /// when unset, or when it holds only whitespace.
    fn preferred_terminal(&self) -> Vec<String>;

    /// `$SHELL`, if it names anything.
    fn login_shell(&self) -> Option<String>;
}

/// Fixed emulator order, tried after `$TERMINAL` has been ruled out.
const FALLBACK_EMULATORS: [&str; 3] = ["x-terminal-emulator", "gnome-terminal", "xterm"];

/// The whole decision tree, kept pure so every branch can be exercised
/// without a real `PATH` or a real terminal window.
///
/// Two independent choices are made and then combined: what to run inside
/// the terminal (a multiplexer session, or failing that a plain shell), and
/// which terminal emulator gets to run it.
pub fn plan(env: &dyn Environment, request: &TerminalRequest) -> Result<Launch, TerminalError> {
    let inner = inner_command(env, &request.session);
    let (program, extra_args) = resolve_emulator(env)?;

    // gnome-terminal is the one common emulator that does not understand
    // `-e`: it wants everything after `--` treated as the command to run,
    // uninterpreted. Every other emulator this reaches for takes `-e`.
    let separator = if is_gnome_terminal(&program) {
        "--"
    } else {
        "-e"
    };

    let mut args = extra_args;
    args.push(separator.to_string());
    args.extend(inner);

    Ok(Launch {
        program,
        args,
        cwd: request.cwd.clone(),
    })
}

/// What to run once a terminal has opened: a multiplexer session if one is
/// installed, otherwise the user's login shell, otherwise a bare `sh`.
///
/// A missing multiplexer is not a failure — it just means fewer of these
/// three branches are reachable, and the last one always is.
fn inner_command(env: &dyn Environment, session: &str) -> Vec<String> {
    if env.find_program("tmux").is_some() {
        vec![
            "tmux".to_string(),
            "new-session".to_string(),
            "-A".to_string(),
            "-s".to_string(),
            session.to_string(),
        ]
    } else if env.find_program("zellij").is_some() {
        vec![
            "zellij".to_string(),
            "attach".to_string(),
            "-c".to_string(),
            session.to_string(),
        ]
    } else if let Some(shell) = env.login_shell() {
        vec![shell]
    } else {
        vec!["sh".to_string()]
    }
}

/// Which terminal emulator to spawn, and any extra arguments `$TERMINAL`
/// carried ahead of the command it is asked to run.
///
/// `$TERMINAL` is tried first and, if it names something that cannot be
/// found, abandoned entirely rather than partially honoured — falling
/// through to the fixed order rather than failing outright, because a stale
/// or typo'd `$TERMINAL` should not make every worktree's terminal button
/// dead.
fn resolve_emulator(env: &dyn Environment) -> Result<(PathBuf, Vec<String>), TerminalError> {
    let preferred = env.preferred_terminal();
    if let Some((name, extra)) = preferred.split_first() {
        if let Some(program) = env.find_program(name) {
            return Ok((program, extra.to_vec()));
        }
    }

    for name in FALLBACK_EMULATORS {
        if let Some(program) = env.find_program(name) {
            return Ok((program, Vec::new()));
        }
    }

    Err(TerminalError::NoTerminal)
}

/// Whether the *resolved* program is gnome-terminal, so `$TERMINAL=gnome-terminal`
/// gets the same argv shape as finding it the ordinary way.
fn is_gnome_terminal(program: &Path) -> bool {
    program.file_name().and_then(|name| name.to_str()) == Some("gnome-terminal")
}

/// The only place in this crate that reads `std::env`.
pub struct SystemEnvironment;

impl SystemEnvironment {
    /// Tried after `PATH`, in this order. Covers install locations that a
    /// minimal graphical session's `PATH` sometimes leaves out — Flatpak and
    /// Snap chief among them.
    fn fixed_dirs() -> Vec<PathBuf> {
        let mut dirs = vec![PathBuf::from("/usr/bin"), PathBuf::from("/usr/local/bin")];
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join(".local/bin"));
        }
        dirs.push(PathBuf::from("/snap/bin"));
        dirs.push(PathBuf::from("/var/lib/flatpak/exports/bin"));
        dirs
    }
}

impl Environment for SystemEnvironment {
    fn find_program(&self, name: &str) -> Option<PathBuf> {
        let path_dirs = std::env::var_os("PATH")
            .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
            .unwrap_or_default();

        path_dirs
            .into_iter()
            .chain(Self::fixed_dirs())
            .map(|dir| dir.join(name))
            .find(|candidate| is_executable_file(candidate))
    }

    fn preferred_terminal(&self) -> Vec<String> {
        std::env::var("TERMINAL")
            .ok()
            .map(|value| value.split_whitespace().map(String::from).collect())
            .unwrap_or_default()
    }

    fn login_shell(&self) -> Option<String> {
        std::env::var("SHELL")
            .ok()
            .filter(|shell| !shell.trim().is_empty())
    }
}

/// `is_file` alone would offer a program with no execute bit, which then
/// fails at spawn time with a confusing "Permission denied" instead of
/// being skipped in favour of the next candidate.
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Resolves and starts terminals against the real machine.
pub struct SystemTerminal {
    env: SystemEnvironment,
}

impl SystemTerminal {
    pub fn new() -> Self {
        Self {
            env: SystemEnvironment,
        }
    }
}

impl Default for SystemTerminal {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalLauncher for SystemTerminal {
    fn resolve(&self, request: &TerminalRequest) -> Result<Launch, TerminalError> {
        plan(&self.env, request)
    }

    fn start(&self, launch: &Launch) -> Result<(), TerminalError> {
        let child = Command::new(&launch.program)
            .args(&launch.args)
            .current_dir(&launch.cwd)
            .spawn()
            .map_err(|error| TerminalError::Spawn {
                program: launch.program.display().to_string(),
                message: error.to_string(),
            })?;

        // Reaped on its own thread rather than here: `start` has to return
        // promptly so the caller's `Task` slot frees up immediately, and the
        // spawned terminal is meant to keep running long after this call
        // does.
        thread::spawn(move || {
            let mut child = child;
            let _ = child.wait();
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::application::TerminalRequest;

    struct FakeEnvironment {
        programs: Vec<String>,
        terminal: Vec<String>,
        shell: Option<String>,
    }

    impl Environment for FakeEnvironment {
        fn find_program(&self, name: &str) -> Option<PathBuf> {
            self.programs
                .iter()
                .any(|program| program == name)
                .then(|| PathBuf::from(format!("/usr/bin/{name}")))
        }

        fn preferred_terminal(&self) -> Vec<String> {
            self.terminal.clone()
        }

        fn login_shell(&self) -> Option<String> {
            self.shell.clone()
        }
    }

    fn env(programs: &[&str]) -> FakeEnvironment {
        FakeEnvironment {
            programs: programs.iter().map(|s| s.to_string()).collect(),
            terminal: Vec::new(),
            shell: None,
        }
    }

    fn request(session: &str) -> TerminalRequest {
        TerminalRequest {
            cwd: PathBuf::from("/home/dev/project"),
            session: session.to_string(),
        }
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn tmux_wins() {
        let environment = env(&["tmux", "zellij", "xterm"]);
        let launch = plan(&environment, &request("feature")).expect("resolved");

        assert_eq!(launch.program, PathBuf::from("/usr/bin/xterm"));
        assert_eq!(
            launch.args,
            strings(&["-e", "tmux", "new-session", "-A", "-s", "feature"])
        );
    }

    #[test]
    fn only_zellij_is_offered() {
        let environment = env(&["zellij", "xterm"]);
        let launch = plan(&environment, &request("feature")).expect("resolved");

        assert_eq!(
            launch.args,
            strings(&["-e", "zellij", "attach", "-c", "feature"])
        );
    }

    #[test]
    fn shell_env_var_is_third() {
        let mut environment = env(&["xterm"]);
        environment.shell = Some("/bin/zsh".to_string());

        let launch = plan(&environment, &request("feature")).expect("resolved");

        assert_eq!(launch.args, strings(&["-e", "/bin/zsh"]));
    }

    #[test]
    fn plain_sh_when_shell_is_unset() {
        let environment = env(&["xterm"]);

        let launch = plan(&environment, &request("feature")).expect("resolved");

        assert_eq!(launch.args, strings(&["-e", "sh"]));
    }

    #[test]
    fn terminal_env_var_beats_the_fixed_order() {
        let mut environment = env(&["kitty", "xterm", "gnome-terminal"]);
        environment.terminal = strings(&["kitty"]);

        let launch = plan(&environment, &request("feature")).expect("resolved");

        assert_eq!(launch.program, PathBuf::from("/usr/bin/kitty"));
    }

    #[test]
    fn terminal_env_var_keeps_its_split_arguments() {
        let mut environment = env(&["kitty"]);
        environment.terminal = strings(&["kitty", "--hold"]);

        let launch = plan(&environment, &request("feature")).expect("resolved");

        assert_eq!(launch.args, strings(&["--hold", "-e", "sh"]));
    }

    #[test]
    fn an_unrunnable_terminal_env_var_falls_through() {
        let mut environment = env(&["xterm"]);
        environment.terminal = strings(&["ghost-term"]);

        let launch = plan(&environment, &request("feature")).expect("resolved");

        assert_eq!(launch.program, PathBuf::from("/usr/bin/xterm"));
        assert_eq!(launch.args, strings(&["-e", "sh"]));
    }

    #[test]
    fn x_terminal_emulator_then_gnome_terminal_then_xterm() {
        let all_three = env(&["x-terminal-emulator", "gnome-terminal", "xterm"]);
        assert_eq!(
            plan(&all_three, &request("feature")).unwrap().program,
            PathBuf::from("/usr/bin/x-terminal-emulator")
        );

        let last_two = env(&["gnome-terminal", "xterm"]);
        assert_eq!(
            plan(&last_two, &request("feature")).unwrap().program,
            PathBuf::from("/usr/bin/gnome-terminal")
        );

        let only_xterm = env(&["xterm"]);
        assert_eq!(
            plan(&only_xterm, &request("feature")).unwrap().program,
            PathBuf::from("/usr/bin/xterm")
        );
    }

    #[test]
    fn nothing_runnable_returns_no_terminal() {
        let environment = env(&[]);

        let error = plan(&environment, &request("feature")).unwrap_err();

        assert!(matches!(error, TerminalError::NoTerminal));
    }

    #[test]
    fn gnome_terminal_gets_a_double_dash_including_via_terminal_env_var() {
        let fixed_order = env(&["gnome-terminal"]);
        assert_eq!(
            plan(&fixed_order, &request("feature")).unwrap().args,
            strings(&["--", "sh"])
        );

        let mut via_env = env(&["gnome-terminal"]);
        via_env.terminal = strings(&["gnome-terminal"]);
        assert_eq!(
            plan(&via_env, &request("feature")).unwrap().args,
            strings(&["--", "sh"])
        );
    }

    #[test]
    fn everything_else_gets_dash_e() {
        let xterm = env(&["xterm"]);
        assert_eq!(plan(&xterm, &request("feature")).unwrap().args[0], "-e");

        let x_terminal_emulator = env(&["x-terminal-emulator"]);
        assert_eq!(
            plan(&x_terminal_emulator, &request("feature"))
                .unwrap()
                .args[0],
            "-e"
        );

        let mut kitty_via_env = env(&["kitty"]);
        kitty_via_env.terminal = strings(&["kitty"]);
        assert_eq!(
            plan(&kitty_via_env, &request("feature")).unwrap().args[0],
            "-e"
        );
    }

    #[test]
    fn cwd_equals_the_worktree_path() {
        let environment = env(&["xterm"]);
        let mut req = request("feature");
        req.cwd = PathBuf::from("/home/dev/some project");

        let launch = plan(&environment, &req).expect("resolved");

        assert_eq!(launch.cwd, PathBuf::from("/home/dev/some project"));
    }

    #[test]
    fn session_name_is_verbatim_including_a_dot() {
        let environment = env(&["tmux", "xterm"]);
        let launch = plan(&environment, &request("release.candidate")).expect("resolved");

        assert_eq!(
            launch.args,
            strings(&["-e", "tmux", "new-session", "-A", "-s", "release.candidate"])
        );
    }

    #[test]
    fn terminal_env_var_with_shell_metacharacters_lands_verbatim_in_argv() {
        let mut environment = env(&["xterm"]);
        environment.terminal = strings(&["xterm", "$(whoami)"]);

        let launch = plan(&environment, &request("feature")).expect("resolved");

        assert!(
            launch.args.contains(&"$(whoami)".to_string()),
            "the metacharacters must survive as one literal argv entry: {:?}",
            launch.args
        );
    }

    #[test]
    fn a_directory_named_a_space_b_yields_exactly_one_dash_s_argument() {
        let environment = env(&["tmux", "xterm"]);
        let launch = plan(&environment, &request("a b")).expect("resolved");

        assert_eq!(
            launch.args,
            strings(&["-e", "tmux", "new-session", "-A", "-s", "a b"]),
            "the space-containing session name must stay one argv entry"
        );
        assert_eq!(
            launch.args.iter().filter(|arg| *arg == "-s").count(),
            1,
            "exactly one -s flag"
        );
    }

    #[test]
    fn a_whitespace_only_terminal_env_var_falls_through() {
        // `TERMINAL="   "` splits to no tokens at all — indistinguishable
        // from being unset — so this must fall through the same way an
        // absent variable does.
        let mut environment = env(&["xterm"]);
        environment.terminal = Vec::new();

        let launch = plan(&environment, &request("feature")).expect("resolved");

        assert_eq!(launch.program, PathBuf::from("/usr/bin/xterm"));
    }
}
