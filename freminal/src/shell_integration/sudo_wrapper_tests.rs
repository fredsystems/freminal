// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Runs the bundled bash, zsh and fish integrations for real and checks the
//! `sudo` wrapper against a fake `sudo` that prints
//! its argv.
//!
//! The wrapper is freminal's own implementation of sudo(8)'s option syntax:
//! `-e` / `--edit` met before the command word (skipping option operands and
//! `NAME=value` words) means an edit and `sudo` is called unchanged, otherwise
//! `--preserve-env=TERMINFO` is added.
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

/// The first executable file called `name` on `PATH`.
fn find_on_path(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
    })
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
    run_sudo_after(shell, env, prelude, "", args)
}

/// As [`run_sudo`], with `postlude` run after sourcing and before `sudo`.
fn run_sudo_after(
    shell: TestShell,
    env: &Env<'_>,
    prelude: &str,
    postlude: &str,
    args: &[&str],
) -> String {
    let tmp = tempfile::tempdir().expect("tempdir");
    let resources = tmp.path().join("resources");
    assert!(!sync_to_disk(&resources).has_errors());

    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).expect("mkdir bin");
    let fake = bin.join("sudo");
    // The fake `sudo` is a generated test fixture, not a shipped script: it
    // names the `sh` found on PATH absolutely, because build sandboxes (the Nix
    // package check phase) have no `/usr/bin/env`.
    let sh = find_on_path("sh").expect("an `sh` on PATH");
    std::fs::write(
        &fake,
        format!(
            "#!{}\nprintf 'ARGV:'\nfor a in \"$@\"; do printf ' [%s]' \"$a\"; done\nprintf '\\n'\n",
            sh.display()
        ),
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
    let command = format!(
        "{prelude}\n{source}\n{postlude}\nsudo {}",
        quoted_args.join(" ")
    );

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

/// The scan must not depend on zsh options that change string subscripts.
#[test]
fn zsh_scan_is_independent_of_ksh_arrays() {
    let shell = TestShell::Zsh;
    if !shell.is_installed() {
        eprintln!("SKIP: zsh is not installed");
        return;
    }
    let ksh = |args: &[&str]| run_sudo_after(shell, &SUDO_ON, "", "setopt KSH_ARRAYS", args);
    assert_eq!(ksh(&["-ke", "f"]), "[-ke] [f]");
    assert_eq!(ksh(&["-u", "root", "-e", "f"]), "[-u] [root] [-e] [f]");
    assert_eq!(ksh(&["--ed", "f"]), "[--ed] [f]");
    assert_eq!(ksh(&["-ue", "root", "f"]), preserved("[-ue] [root] [f]"));
    assert_eq!(
        ksh(&["--us", "root", "ls"]),
        preserved("[--us] [root] [ls]")
    );
    assert_eq!(ksh(&["ls"]), preserved("[ls]"));
    assert_eq!(
        ksh(&["-h", "localhost", "-e", "f"]),
        "[-h] [localhost] [-e] [f]"
    );
    assert_eq!(
        ksh(&["-kh", "host", "-e", "f"]),
        preserved("[-kh] [host] [-e] [f]")
    );
}

/// `sudo <args>` with the whole-argv expectation: the flag is added, or not.
fn expect_argv(shell: TestShell, args: &[&str], flag_added: bool) {
    let rest = args
        .iter()
        .map(|a| format!("[{a}]"))
        .collect::<Vec<_>>()
        .join(" ");
    let expected = if flag_added { preserved(&rest) } else { rest };
    assert_eq!(
        run_sudo(shell, &SUDO_ON, "", args),
        expected,
        "{shell:?}: sudo {args:?}"
    );
}

#[test]
fn commands_get_preserve_env() {
    let cases: [&[&str]; 23] = [
        &["ls"],
        &["ls", "-l", "/tmp"],
        &["-u", "root", "ls"],
        &["VAR=1", "cmd"],
        &["--", "ls"],
        // The scan stops at the command word, so a later -e is the command's.
        &["cmd", "-e", "file"],
        &["cmd", "--edit", "file"],
        // `--` ends the options: a later -e is not an option of sudo.
        &["--", "-e", "file"],
        // `e` is `u`'s value (a user named "e"); `root` is the command.
        &["-ue", "root", "f"],
        // An option's operand is not mistaken for an edit.
        &["--user", "e", "ls"],
        &["--user=e", "ls"],
        &["-u", "-e", "ls"],
        &["-g", "wheel", "VAR=1", "ls"],
        &["-n", "ls"],
        // `h` takes its value only from the rest of its own word.
        &["-hfoo", "ls"],
        // `--login` is an exact name even though it prefixes `login-class`.
        &["--login", "ls"],
        // Ambiguous prefix: unknown, takes no value.
        &["--re", "ls"],
        // `--preserve-env` takes a value only with `=`.
        &["--preserve-env", "ls"],
        &["--preserve-env=X", "ls"],
        // `-a` requires a value, which is not mistaken for the command.
        &["-a", "x", "ls"],
        // A standalone `-h` takes the next word as the host, so `ls` is not a
        // command here and no edit follows; an assignment is not a host.
        &["-h", "ls"],
        &["-h", "VAR=1", "ls"],
        // Grouped `-kh` takes no detached host: `host` is the command.
        &["-kh", "host", "-e", "f"],
    ];
    for_each_shell(|shell| {
        for args in cases {
            expect_argv(shell, args, true);
        }
    });
}

#[test]
fn edit_invocations_are_left_unchanged() {
    let cases: [&[&str]; 25] = [
        &["-e", "f"],
        &["--edit", "f"],
        &["-u", "root", "-e", "f"],
        &["--user=root", "-e", "f"],
        &["--user", "root", "-e", "f"],
        &["-ke", "f"],
        &["-n", "-e", "f"],
        &["VAR=1", "-e", "f"],
        &["-uroot", "-e", "f"],
        &["-ku", "root", "--edit", "f"],
        &["--chdir", "/tmp", "--edit", "f"],
        &["-e", "-u", "root", "f"],
        // Unique long-option prefixes.
        &["--ed", "f"],
        &["--edi", "f"],
        &["--us", "root", "-e", "f"],
        &["--ho", "h", "--edit", "f"],
        &["--login-c", "c", "-e", "f"],
        // Value-required options consume the next word, `h` does not.
        &["-h", "-e", "f"],
        &["-h", "localhost", "-e", "f"],
        &["-hfoo", "-e", "f"],
        // A word with `=` that starts with `/` is a host, not an assignment.
        &["-h", "/a=b", "-e", "f"],
        &["-a", "x", "-e", "f"],
        &["-c", "cls", "-e", "f"],
        &["--auth-type", "x", "-e", "f"],
        &["--login-class", "c", "-e", "f"],
    ];
    for_each_shell(|shell| {
        for args in cases {
            expect_argv(shell, args, false);
        }
    });
}

#[test]
fn feature_list_is_matched_by_whole_item() {
    let on = [
        "sudo",
        "cursor:blink,sudo,title",
        "sudo,title",
        "title,sudo",
    ];
    let off = [
        "nosudo",
        "sudo2",
        "cursor:blink",
        "sudo:x",
        "mysudo,sudoer",
        "",
    ];
    for_each_shell(|shell| {
        for features in on {
            let env = Env {
                features: Some(features),
                terminfo: Some("/pane/terminfo"),
            };
            assert_eq!(
                run_sudo(shell, &env, "", &["ls"]),
                preserved("[ls]"),
                "{shell:?}: features {features:?}"
            );
        }
        for features in off {
            let env = Env {
                features: Some(features),
                terminfo: Some("/pane/terminfo"),
            };
            assert_eq!(
                run_sudo(shell, &env, "", &["ls"]),
                "[ls]",
                "{shell:?}: features {features:?}"
            );
        }
    });
}

#[test]
fn feature_absent_leaves_sudo_alone() {
    let env = Env {
        features: None,
        terminfo: Some("/pane/terminfo"),
    };
    for_each_shell(|shell| {
        assert_eq!(run_sudo(shell, &env, "", &["ls"]), "[ls]", "{shell:?}");
    });
}

#[test]
fn no_terminfo_means_no_wrapper() {
    let unset = Env {
        features: Some("sudo"),
        terminfo: None,
    };
    let empty = Env {
        features: Some("sudo"),
        terminfo: Some(""),
    };
    for_each_shell(|shell| {
        assert_eq!(run_sudo(shell, &unset, "", &["ls"]), "[ls]", "{shell:?}");
        assert_eq!(run_sudo(shell, &empty, "", &["ls"]), "[ls]", "{shell:?}");
    });
}

/// The fish integration leaves an existing `sudo` function or alias alone.
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
    assert_eq!(
        run_sudo(
            shell,
            &SUDO_ON,
            "alias sudo 'echo ARGV: [aliased]'",
            &["ls"]
        ),
        "[aliased] ls"
    );
}

/// The wrapper is freminal's own code: its block cites no third-party source.
#[test]
fn sudo_blocks_name_no_third_party_source() {
    let tmp = tempfile::tempdir().expect("tempdir");
    assert!(!sync_to_disk(tmp.path()).has_errors());
    for shell in TestShell::ALL {
        let text = std::fs::read_to_string(Path::new(tmp.path()).join(shell.script()))
            .expect("read script");
        let start = text
            .find("sudo keeps TERMINFO")
            .unwrap_or_else(|| panic!("{shell:?}: no sudo block"));
        // The block ends at the first line that closes the top-level `if`.
        let end_line = match shell {
            TestShell::Fish => "\n    end\n",
            TestShell::Bash | TestShell::Zsh => "\nfi\n",
        };
        let block = &text[start..];
        let block = &block[..block.find(end_line).expect("sudo block end")];
        let lower = block.to_lowercase();
        for forbidden in ["ghostty", "kitty", "github.com"] {
            assert!(
                !lower.contains(forbidden),
                "{shell:?}: sudo block mentions {forbidden:?}"
            );
        }
    }
}
