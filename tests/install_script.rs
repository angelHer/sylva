//! Tests for the desktop entry `install.sh` writes.
//!
//! GIO drops a `.desktop` entry whose `Exec` binary it cannot resolve on PATH,
//! and a GNOME Wayland session never reads `~/.profile`, so `~/.local/bin` is
//! on the session PATH of some machines and not others. On the ones where it
//! is missing the launcher does not merely fail to start — it never appears in
//! the applications menu at all. The installed entry therefore has to name the
//! binary by absolute path, which is what these tests pin down.

use std::path::PathBuf;
use std::process::Command;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Renders the entry the way `install.sh` would for the given binary directory,
/// without building or installing anything.
fn rendered_entry(bin_dir: &str) -> String {
    let root = repository_root();
    let output = Command::new("sh")
        .arg(root.join("install.sh"))
        .arg("--print-desktop-entry")
        .arg(bin_dir)
        .current_dir(&root)
        .output()
        .expect("run install.sh");

    assert!(
        output.status.success(),
        "install.sh --print-desktop-entry failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout).expect("utf-8 output")
}

#[test]
fn exec_names_the_binary_by_absolute_path() {
    let entry = rendered_entry("/opt/sylva/bin");

    assert!(
        entry
            .lines()
            .any(|line| line == "Exec=/opt/sylva/bin/sylva --welcome %f"),
        "no absolute Exec line in:\n{entry}"
    );
}

#[test]
fn no_bare_exec_line_survives_the_substitution() {
    let entry = rendered_entry("/opt/sylva/bin");

    assert!(
        !entry.lines().any(|line| line == "Exec=sylva --welcome %f"),
        "the template's bare Exec line was left behind in:\n{entry}"
    );
}

#[test]
fn a_bin_dir_with_spaces_is_quoted() {
    let entry = rendered_entry("/home/some one/.local/bin");

    // Unquoted, the shell that GIO spawns would read the entry as the command
    // `/home/some` with `one/.local/bin/sylva` as its first argument.
    assert!(
        entry
            .lines()
            .any(|line| line == r#"Exec="/home/some one/.local/bin/sylva" --welcome %f"#),
        "path with a space was not quoted in:\n{entry}"
    );
}

#[test]
fn the_rest_of_the_template_is_preserved() {
    let entry = rendered_entry("/opt/sylva/bin");

    // StartupWMClass pairs the window with the launcher and MimeType is what
    // puts sylva in the file manager's "Open With" menu; a substitution that
    // dropped either would break things no Exec assertion would notice.
    for expected in [
        "Type=Application",
        "Name=sylva",
        "Icon=sylva",
        "MimeType=inode/directory;",
        "StartupWMClass=sylva",
    ] {
        assert!(
            entry.lines().any(|line| line == expected),
            "template lost `{expected}` in:\n{entry}"
        );
    }
}
