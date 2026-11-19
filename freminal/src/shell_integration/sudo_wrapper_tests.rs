// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Runs the bundled bash, zsh and fish integrations for real and checks the
//! `sudo` wrapper (Task 151.12) against a fake `sudo` that prints its argv.
//!
//! The expectations are Ghostty's scan rule (`ghostty.bash`,
//! `ghostty-integration`, `ghostty-shell-integration.fish`): arguments are
//! scanned up to the first one that is neither an option (`-*`) nor an
//! assignment (`*=*`); `-e` / `--edit` met on the way means sudoedit and `sudo`
//! is called unchanged, otherwise `--preserve-env=TERMINFO` is added.
//!
//! A shell that is not installed is skipped with a visible message.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use super::sync_to_disk;

const PRESERVED: &str = "[--preserve-env=TERMINFO]";

#[derive(Clone, Copy, Debug)]
enum TestShell {
    Bash,
    Zsh,
    Fish,
}

impl TestShell {
    const ALL: [Self; 3] = [Self::Bash, Self::Zsh, Self::Fish];

    const fn program(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Zsh => "zsh",
            Self::Fish => "fish",
        }
    }

    const fn script(self) -> &'static str {
        match self {
            Self::Bash => "bash/freminal-init.bash",
            Self::Zsh => "zsh/freminal-integration",
            Self::Fish => "fish/vendor_conf.d/freminal.fish",
        }
    }

    fn is_installed(self) -> bool {
        Command::new(self.program())
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }
}

/// Environment the scripts see: `None` leaves the variable unset.
struct Env<'a> {
    features: Option<&'a str>,
    terminfo: Option<&'a str>,
}

const SUDO_ON: Env<'static> = Env {
    features: Some("sudo"),
    terminfo: Some("/pane/terminfo"),
};

fn quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', "'\\''"))
}

/// Source the integration under `shell`, run `sudo <args>` (optionally after
/// `prelude`), and return the argv the fake `sudo` printed, as `[a] [b] ...`.
fn run_sudo(shell: TestShell, env: &Env<'_>, prelude: &str, args: &[&str]) -> String {
    let tmp = tempfile::tempdir().expect("tempdir");
    let resources = tmp.path().join("resources");
    assert!(!sync_to_disk(&resources).has_errors());

    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).expect("mkdir bin");
    let fake = bin.join("sudo");
    std::fs::write(
        &fake,
        "#!/usr/bin/env sh\nprintf 'ARGV:'\nfor a in \"$@\"; do printf ' [%s]' \"$a\"; done\nprintf '\\n'\n",
    )
    .expect("write fake sudo");
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).expect("mkdir home");

    let script = resources.join(shell.script());
    let quoted_args: Vec<String> = args.iter().map(|a| quote(a)).collect();
    let source = if matches!(shell, TestShell::Bash | TestShell::Zsh) {
        format!(". {}", quote(&script.display().to_string()))
    } else {
        format!("source {}", quote(&script.display().to_string()))
    };
    let command = format!("{prelude}\n{source}\nsudo {}", quoted_args.join(" "));

    let mut cmd = Command::new(shell.program());
    match shell {
        TestShell::Bash => cmd.args(["--norc", "--noprofile", "-c", &command]),
        TestShell::Zsh => cmd.args(["-f", "-c", &command]),
        TestShell::Fish => cmd.args(["--no-config", "-c", &command]),
    };
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    cmd.env("PATH", path)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("TERM_PROGRAM", "freminal")
        .env_remove("TERMINFO")
        .env_remove("FREMINAL_SHELL_FEATURES");
    if let Some(features) = env.features {
        cmd.env("FREMINAL_SHELL_FEATURES", features);
    }
    if let Some(terminfo) = env.terminfo {
        cmd.env("TERMINFO", terminfo);
    }

    let out = cmd.output().expect("run shell");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    // The integration also prints OSC markers; keep only the fake sudo's line.
    stdout
        .lines()
        .find_map(|l| {
            l.find("ARGV:")
                .map(|i| l[i + "ARGV:".len()..].trim().to_owned())
        })
        .unwrap_or_else(|| {
            panic!(
                "{shell:?}: fake sudo was not run. stdout: {stdout:?} stderr: {:?}",
                String::from_utf8_lossy(&out.stderr)
            )
        })
}

/// Run `check` for every installed shell; say so when one is skipped.
fn for_each_shell(check: impl Fn(TestShell)) {
    let mut ran = 0;
    for shell in TestShell::ALL {
        if shell.is_installed() {
            check(shell);
            ran += 1;
        } else {
            eprintln!("SKIP: {} is not installed", shell.program());
        }
    }
    if ran == 0 {
        eprintln!("SKIP: none of bash, zsh or fish is installed");
    }
}

fn preserved(rest: &str) -> String {
    format!("{PRESERVED} {rest}")
}

#[test]
fn plain_command_gets_preserve_env() {
    for_each_shell(|shell| {
        assert_eq!(
            run_sudo(shell, &SUDO_ON, "", &["ls"]),
            preserved("[ls]"),
            "{shell:?}"
        );
        assert_eq!(
            run_sudo(shell, &SUDO_ON, "", &["ls", "-l", "/tmp"]),
            preserved("[ls] [-l] [/tmp]"),
            "{shell:?}"
        );
    });
}

#[test]
fn sudoedit_flags_are_left_unchanged() {
    for_each_shell(|shell| {
        assert_eq!(
            run_sudo(shell, &SUDO_ON, "", &["-e", "file"]),
            "[-e] [file]",
            "{shell:?}"
        );
        assert_eq!(
            run_sudo(shell, &SUDO_ON, "", &["--edit", "file"]),
            "[--edit] [file]",
            "{shell:?}"
        );
        // An option that takes no operand before -e keeps scanning.
        assert_eq!(
            run_sudo(shell, &SUDO_ON, "", &["-n", "-e", "file"]),
            "[-n] [-e] [file]",
            "{shell:?}"
        );
        // An assignment before -e keeps scanning.
        assert_eq!(
            run_sudo(shell, &SUDO_ON, "", &["FOO=1", "-e", "file"]),
            "[FOO=1] [-e] [file]",
            "{shell:?}"
        );
    });
}

/// Ghostty's scan stops at the first argument that is neither an option nor an
/// assignment, and an option's operand (`root` in `-u root`) is such an
/// argument.  So `sudo -u root -e file` is NOT recognised as sudoedit by
/// Ghostty, and this wrapper reproduces that exactly.  Pinned here so a
/// future deviation from Ghostty is deliberate.
#[test]
fn option_operand_ends_the_scan_exactly_as_ghostty_does() {
    for_each_shell(|shell| {
        assert_eq!(
            run_sudo(shell, &SUDO_ON, "", &["-u", "root", "-e", "file"]),
            preserved("[-u] [root] [-e] [file]"),
            "{shell:?}"
        );
        assert_eq!(
            run_sudo(shell, &SUDO_ON, "", &["-e", "-u", "root", "file"]),
            "[-e] [-u] [root] [file]",
            "{shell:?}"
        );
    });
}

#[test]
fn assignment_before_the_command_is_scanned_past() {
    for_each_shell(|shell| {
        assert_eq!(
            run_sudo(shell, &SUDO_ON, "", &["VAR=1", "cmd"]),
            preserved("[VAR=1] [cmd]"),
            "{shell:?}"
        );
        // The scan has stopped at `cmd`, so a later -e is the command's own.
        assert_eq!(
            run_sudo(shell, &SUDO_ON, "", &["cmd", "-e", "file"]),
            preserved("[cmd] [-e] [file]"),
            "{shell:?}"
        );
    });
}

#[test]
fn feature_list_is_matched_by_name() {
    let env = Env {
        features: Some("cursor:blink,sudo,title"),
        terminfo: Some("/pane/terminfo"),
    };
    for_each_shell(|shell| {
        assert_eq!(
            run_sudo(shell, &env, "", &["ls"]),
            preserved("[ls]"),
            "{shell:?}"
        );
    });
}

#[test]
fn feature_off_leaves_sudo_alone() {
    let unrelated = Env {
        features: Some("cursor:blink"),
        terminfo: Some("/pane/terminfo"),
    };
    let absent = Env {
        features: None,
        terminfo: Some("/pane/terminfo"),
    };
    for_each_shell(|shell| {
        assert_eq!(
            run_sudo(shell, &unrelated, "", &["ls"]),
            "[ls]",
            "{shell:?}"
        );
        assert_eq!(run_sudo(shell, &absent, "", &["ls"]), "[ls]", "{shell:?}");
    });
}

#[test]
fn no_terminfo_means_no_wrapper() {
    let env = Env {
        features: Some("sudo"),
        terminfo: None,
    };
    for_each_shell(|shell| {
        assert_eq!(run_sudo(shell, &env, "", &["ls"]), "[ls]", "{shell:?}");
    });
}

/// Ghostty's fish integration leaves an existing `sudo` function or alias
/// alone (bash and zsh define theirs unconditionally, as Ghostty does).
#[test]
fn fish_does_not_clobber_an_existing_sudo_function() {
    let shell = TestShell::Fish;
    if !shell.is_installed() {
        eprintln!("SKIP: fish is not installed");
        return;
    }
    assert_eq!(
        run_sudo(
            shell,
            &SUDO_ON,
            "function sudo; echo \"ARGV: [user-defined]\"; end",
            &["ls"]
        ),
        "[user-defined]"
    );
}

#[test]
fn shipped_scripts_cite_ghostty() {
    let tmp = tempfile::tempdir().expect("tempdir");
    assert!(!sync_to_disk(tmp.path()).has_errors());
    for shell in TestShell::ALL {
        let text = std::fs::read_to_string(Path::new(tmp.path()).join(shell.script()))
            .expect("read script");
        assert!(
            text.contains("github.com/ghostty-org/ghostty/blob/"),
            "{shell:?} integration must cite the Ghostty source it reproduces"
        );
    }
}
