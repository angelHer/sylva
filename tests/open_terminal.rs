//! Integration tests for the terminal launch path, run against the real
//! machine. These skip, rather than fail, when a tool they need is not
//! installed — the same convention tests/worktree_ops.rs follows for `git`.

use std::path::Path;
use std::thread;
use std::time::Duration;
use std::{fs, process::Command};

use sylva::application::{Launch, TerminalLauncher};
use sylva::infrastructure::terminal::{Environment, SystemEnvironment};
use sylva::infrastructure::SystemTerminal;

#[test]
fn a_program_on_path_resolves_and_a_nonsense_name_does_not() {
    if std::env::var_os("PATH").is_none() {
        eprintln!("skipping: PATH is unset");
        return;
    }

    let environment = SystemEnvironment;

    assert!(
        environment.find_program("sh").is_some(),
        "sh is expected on any machine able to run this test suite"
    );
    assert!(environment
        .find_program("definitely-not-a-real-program-xyz")
        .is_none());
}

#[test]
fn starting_a_launch_runs_it_in_the_given_directory_and_leaves_no_zombie() {
    if !Path::new("/bin/sh").exists() {
        eprintln!("skipping: /bin/sh is not installed");
        return;
    }

    let dir = tempfile::tempdir().expect("temp dir");
    let launch = Launch {
        program: std::path::PathBuf::from("/bin/sh"),
        args: vec!["-c".to_string(), "pwd > out".to_string()],
        cwd: dir.path().to_path_buf(),
    };

    SystemTerminal::new()
        .start(&launch)
        .expect("the launch starts");

    // `start` hands the child to its own reaper thread rather than waiting
    // here, so the file it writes and its own reaping both happen off this
    // thread — give them a moment rather than racing them.
    let out = dir.path().join("out");
    let mut contents = String::new();
    for _ in 0..100 {
        if let Ok(text) = fs::read_to_string(&out) {
            if !text.trim().is_empty() {
                contents = text;
                break;
            }
        }
        thread::sleep(Duration::from_millis(20));
    }

    assert_eq!(
        contents.trim(),
        dir.path().to_str().expect("a UTF-8 temp path"),
        "the command ran with the requested directory as its cwd"
    );

    thread::sleep(Duration::from_millis(100));
    assert_eq!(
        zombie_children_of_this_process("sh"),
        0,
        "the spawned sh must have been wait()ed on by the reaper thread"
    );
}

/// Counts zombie processes whose parent is this test binary and whose name
/// matches — a child left un-`wait()`ed shows up exactly this way.
fn zombie_children_of_this_process(comm: &str) -> usize {
    let output = Command::new("ps")
        .args(["-eo", "ppid,stat,comm"])
        .output()
        .expect("ps must be available to check for zombies");
    let my_pid = std::process::id().to_string();

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .skip(1)
        .filter(|line| {
            let mut fields = line.split_whitespace();
            let ppid = fields.next().unwrap_or("");
            let stat = fields.next().unwrap_or("");
            let name = fields.next().unwrap_or("");
            ppid == my_pid && stat.contains('Z') && name == comm
        })
        .count()
}
