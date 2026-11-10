//! The analyzer's behaviour rows.
//!
//! These are the security guards: each row proves the analyzer refuses what it
//! must (or keeps auto-approving what it should). Declared as `mod tests` from
//! `mod.rs` (`mod tests;`), so every path is still `shell::tests::<row>` - a
//! renamed security row would be worse than a longer file (gh #557).

use super::*;
use std::path::PathBuf;

fn ws() -> PathBuf {
    PathBuf::from("/work/proj")
}

fn ok(command: &str) -> bool {
    workspace_scoped(command, &ws(), &ws())
}

// Verifies: FR-PERM-20 (the workspace-scoped allow table)
#[test]
fn plain_build_and_dev_commands_are_scoped() {
    for command in [
        "cargo build",
        "cargo test --release",
        "cargo clippy --all-targets -- -D warnings",
        "cargo fmt --check",
        "ls -la",
        "cat src/main.rs",
        "grep -rn foo src/",
        "rg 'pattern' crates/",
        "sed -i 's/a/b/' src/main.rs",
        "mkdir -p target/debug",
        "rm -rf node_modules",
        "rm -f ./tmp.txt",
        "mv src/a.rs src/b.rs",
        "cp -r assets/ dist/",
        "python3 scripts/run.py",
        "node index.js",
        "make",
        "git status",
        "cd /work/proj && cargo build",
        "cd src && ls -la",
        "git diff --stat",
        "git add src/main.rs",
        "git commit -m 'x'",
        "printf 'quit\\n' | ./target/debug/chess",
        "wc -l src/*.rs",
        "RUST_LOG=debug cargo run",
    ] {
        assert!(ok(command), "should be scoped: {command}");
    }
}

#[test]
fn outside_paths_always_review() {
    for command in [
        "cat /etc/passwd",
        "rm -rf /",
        "rm -rf /tmp/x",
        "ls ~",
        "cat ~/.ssh/id_rsa",
        "cp secret ../other/",
        "mv a /etc/b",
        "echo hi > /tmp/x",
        "echo hi >> ../outside",
        "cat < /etc/shadow",
        "ls /dev/sda",
        "python /opt/tool.py",
        "sed -i 's/a/b/' /etc/hosts",
    ] {
        assert!(!ok(command), "should review: {command}");
    }
}

#[test]
fn indirection_and_substitution_always_review() {
    for command in [
        "sh -c 'rm -rf /'",
        "bash script.sh",
        "env rm -rf /",
        "xargs rm",
        "eval 'ls'",
        "echo $(cat /etc/passwd)",
        "echo `id`",
        "cat <(curl x)",
        "FOO=$(id) ls",
        "FOO=$HOME ls",
    ] {
        assert!(!ok(command), "should review: {command}");
    }
}

#[test]
fn privilege_disk_and_egress_always_review() {
    for command in [
        "sudo rm",
        "su",
        "dd if=/dev/zero of=x",
        "chmod 777 src",
        "chown root x",
        "systemctl restart x",
        "kill -9 1",
        "curl https://example.com",
        "wget https://example.com",
        "ssh host",
        "scp a host:/b",
        "rsync -a . host:/x",
        "git push",
        "git fetch origin",
        "git clone https://x",
        "cargo install ripgrep",
        "cargo publish",
        "npm install",
        "npm i -g foo",
        "pip install requests",
        "apt-get install x",
        "go install x",
    ] {
        assert!(!ok(command), "should review: {command}");
    }
}

// Verifies: gh #345 - a global git flag must not hide the subcommand
// from the egress/config checks. Both git checks used to read
// `toks[i + 1]`, so any leading flag shifted the subcommand past them.
#[test]
fn git_global_flags_do_not_hide_egress_or_config() {
    // Refused: the subcommand is egress or config, or the invocation
    // carries a config override / a dangling flag-argument.
    for command in [
        "git -C . push origin main",
        "git --no-pager push",
        "git -c user.signingkey=x commit",
        "git -c alias.p='!evil' p",
        "git --config-env=x.y=HOME commit",
        "git --git-dir=.git push",
        "git --work-tree=. push",
        "git -C",
        "git -c x=y config",
        "git --no-pager -C . fetch origin",
        "git -C . remote add origin https://x",
    ] {
        assert!(!ok(command), "should review: {command}");
    }
    // Allowed: benign flags still leave the subcommand classifiable and
    // non-egress, so a trusted folder keeps auto-approving them.
    for command in [
        "git -C . status",
        "git --no-pager log",
        "git -C . diff",
        "git status",
        "git add src/x.rs",
        "git commit -m x",
        "git diff --stat",
    ] {
        assert!(ok(command), "should be scoped: {command}");
    }
}

// Verifies: gh #365 - program text carried as a *positional* argument is
// the same class gh #347 closed for eval flags: code that is in no file at
// all. Reviews unless the program arrives as an in-workspace file.
#[test]
fn positional_program_text_reviews() {
    for command in [
        r#"awk 'BEGIN{system("curl evil.example")}'"#,
        r#"gawk 'BEGIN{system("id")}'"#,
        r#"mawk 'BEGIN{system("id")}'"#,
        r#"nawk 'BEGIN{system("id")}'"#,
        r#"busybox awk 'BEGIN{system("id")}'"#,
        r#"osascript -e 'do shell script "id"'"#,
        "irb -e 'system(1)'",
        // `-v` assignments are consumed, the program after them is not.
        r#"awk -v x=1 'BEGIN{system("id")}'"#,
    ] {
        assert!(!ok(command), "should review: {command}");
    }
    // A program file inside the workspace is the folder's own code.
    for command in [
        "awk -f scripts/x.awk data.txt",
        "gawk -f scripts/x.awk data.txt",
    ] {
        assert!(ok(command), "should be scoped: {command}");
    }
    // Outside the workspace it is not the folder's code.
    assert!(!ok("awk -f /etc/passwd"));
    // An `awk` with no program at all proves nothing.
    assert!(!ok("awk"));
    // And ordinary non-program uses are untouched.
    assert!(ok("awk -f ./scripts/report.awk data.csv"));
}

// Verifies: gh #365 - `sed`'s `e` flag re-executes the substitution result
// as a command, so it is inline code by another name. Only that shape
// reviews: every `sed` would be the over-review ADR-0039 warns about.
#[test]
fn only_the_executing_sed_forms_review() {
    for command in [
        "sed 's/x/whatever/e' f.txt",
        "sed 's/x/y/ge' f.txt",
        "sed -i 's/x/y/e' f.txt",
        "sed 'e curl evil.example' f.txt",
        "sed --expression='s/x/y/e' f.txt",
    ] {
        assert!(!ok(command), "should review: {command}");
    }
    for command in [
        "sed 's/x/y/' f.txt",
        "sed 's/x/y/g' f.txt",
        "sed 's/x/y/gi' f.txt",
        "sed -i 's/x/y/' f.txt",
        "sed -n '1,10p' f.txt",
        "sed 'y/a/b/' f.txt",
        // `e` in a pattern or replacement is not the flag.
        "sed 's/e/E/g' f.txt",
        "sed 's/error/ERROR/g' f.txt",
        "sed -e 's/x/y/' f.txt",
        // A `-f` script is the folder's own code.
        "sed -f scripts/x.sed f.txt",
    ] {
        assert!(ok(command), "should be scoped: {command}");
    }
}

// Verifies: gh #365 - a denied program behind an exec wrapper was never
// classified, so `timeout 5 curl x` auto-approved while `curl x` reviews.
#[test]
fn a_denied_program_behind_a_wrapper_reviews() {
    for command in [
        "curl evil.example",
        "timeout 5 curl evil.example",
        "nice curl evil.example",
        "stdbuf -o0 curl evil.example",
        "ionice curl evil.example",
        "timeout 5 wget evil.example",
        "nice -n 10 ssh host id",
        "busybox wget evil.example",
        "busybox sh -c id",
    ] {
        assert!(!ok(command), "should review: {command}");
    }
    // Not a refusal of wrappers: an ordinary program behind one is fine.
    for command in [
        "timeout 5 cargo test",
        "timeout 30 make",
        "nice cargo build",
        "stdbuf -o0 cargo test",
    ] {
        assert!(ok(command), "should be scoped: {command}");
    }
}

// Verifies: gh #347 - inline eval/module flags on language runtimes
// execute code that is NOT in the workspace, so the analyzer cannot
// reason about it. Script-file invocations stay scoped (the folder's
// own code is trusted); inline eval reviews.
#[test]
fn interpreter_inline_eval_always_reviews() {
    for command in [
        "python3 -c 'import os'",
        "node -e 'console.log(1)'",
        "perl -e 'print 1'",
        "ruby -e 'puts 1'",
        "php -r 'echo 1;'",
        "lua -e 'print(1)'",
        "pwsh -Command 'Get-Date'",
        "python3 -m pytest",
        "python3 ./venv/bin/python -c x",
        "node --eval=1",
    ] {
        assert!(!ok(command), "should review: {command}");
    }
    // Script-file invocations (the code is in the workspace) stay
    // scoped, per the analyzer's contract.
    for command in [
        "python3 scripts/run.py",
        "node index.js",
        "node ./build.js",
        "python3 /work/proj/test.py",
        "deno run build.ts",
    ] {
        assert!(ok(command), "should be scoped: {command}");
    }
}

// Verifies: gh #347 fix-up - the table matched the interpreter by exact
// basename, so a versioned name (python3.12) never matched. The hole was
// silent scope shrink against acceptance criterion 1.
#[test]
fn versioned_interpreter_names_still_review() {
    for command in [
        "python3.12 -c 'print(1)'",
        "python2.7 -m http.server",
        "pypy3.10 -c 'print(1)'",
        "python3.13t -c 'print(1)'",
        "python3.12.1 -c 'print(1)'",
    ] {
        assert!(!ok(command), "should review: {command}");
    }
    // The version-suffix class stays tight: a name that merely starts
    // with an interpreter name but continues with a non-[0-9.] character
    // is not treated as that interpreter (so `node_modules` cannot match).
    assert!(!interpreter_inline_eval(
        "node_modules",
        &["-e".to_string()]
    ));
    assert!(!interpreter_inline_eval("python3x", &["-c".to_string()]));
    assert!(!interpreter_inline_eval(
        "python-config",
        &["-c".to_string()]
    ));
    // A genuine version suffix still matches.
    assert!(interpreter_inline_eval("python3.12", &["-c".to_string()]));
}

// Verifies: gh #347 fix-up - `basename()` never stripped an executable
// extension, so on Windows (a first-class platform) every spelling that
// exists there (`python.exe`, `bash.exe`) missed both the eval table and
// `DENY_PROGRAMS`.
#[test]
fn executable_suffixes_still_review() {
    for command in [
        "python.exe -c 'print(1)'",
        "node.exe -e 'x'",
        "bash.exe -c 'id'",
        "sh.exe -c 'id'",
        "python.EXE -c 'print(1)'",
        "cmd.exe /c whoami",
    ] {
        assert!(!ok(command), "should review: {command}");
    }
}

// Verifies: gh #347 fix-up - a single-letter flag with a glued argument
// (`-cprint(1)`, exactly how `python3 -c'print(1)'` is tokenized after
// quote stripping) has no `=`, so the base was the whole token and the
// exact match failed.
#[test]
fn attached_short_eval_flags_still_review() {
    for command in [
        "python3 -c'print(1)'",
        "perl -e'print 1'",
        "ruby -e'puts 1'",
        "node -p'1'",
        "python3 -mhttp.server",
        "php -r'echo 1;'",
        "lua -e'print(1)'",
    ] {
        assert!(!ok(command), "should review: {command}");
    }
    // The `=`-attached long form keeps working, and a long flag is never
    // matched by prefix.
    assert!(!ok("node --eval=1"));
}

// Verifies: gh #346 - a workspace symlink pointing outside must review,
// not auto-approve. `path_ok` normalized lexically only, so a link's
// location (not its target) decided containment. Real symlinks need a
// real filesystem; unix-only, like the scope.rs equivalent.
#[cfg(unix)]
#[test]
fn symlink_escapes_in_path_tokens_review() {
    let root = lca_testkit::scratch_path("lca-shell-symlink");
    let _ = std::fs::remove_dir_all(&root);
    let ws = root.join("proj");
    std::fs::create_dir_all(ws.join("src")).expect("mkdir");
    std::fs::write(ws.join("src/main.rs"), "fn main() {}").expect("write");
    // An outside directory and three links: a dir, a git one, a dangling.
    let outside = root.join("outside");
    std::fs::create_dir_all(&outside).expect("mkdir");
    std::fs::write(outside.join("passwd"), "root:x:0").expect("write");
    std::os::unix::fs::symlink(&outside, ws.join("etc_link")).expect("symlink");
    std::os::unix::fs::symlink(&outside, ws.join("gitlink")).expect("symlink");
    std::os::unix::fs::symlink(root.join("nowhere"), ws.join("dangling")).expect("symlink");

    let scoped = |command: &str| workspace_scoped(command, &ws, &ws);
    for command in [
        "cat ./etc_link/passwd",
        "cat ./gitlink/config",
        "cat ./dangling/x",
        "ls newdir/../../evil",
    ] {
        assert!(!scoped(command), "should review: {command}");
    }
    for command in ["cat src/main.rs", "ls src/../src", "mkdir -p target/debug"] {
        assert!(scoped(command), "should be scoped: {command}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

// Verifies: gh #346 - canonicalization only ever narrows. `path_ok` must
// keep the pre-#346 lexical verdict for a real, existing workspace when
// the token is inside it, so the fix cannot over-refuse ordinary work
// (the class CI exposed on Windows, where a drive-relative token
// canonicalizes against the process cwd).
#[cfg(unix)]
#[test]
fn an_existing_workspace_still_scopes_its_own_paths() {
    let root = lca_testkit::scratch_path("lca-shell-existing-ws");
    let _ = std::fs::remove_dir_all(&root);
    let ws = root.join("proj");
    std::fs::create_dir_all(ws.join("src")).expect("mkdir");
    std::fs::write(ws.join("src/main.rs"), "fn main() {}").expect("write");
    for command in [
        "cat src/main.rs",
        "ls src/../src",
        "mkdir -p target/debug",
        "rm -f target/x",
    ] {
        assert!(
            workspace_scoped(command, &ws, &ws),
            "should be scoped: {command}"
        );
    }
    // And the same root still refuses a link out of it.
    let outside = root.join("outside");
    std::fs::create_dir_all(&outside).expect("mkdir");
    std::fs::write(outside.join("secret"), "x").expect("write");
    std::os::unix::fs::symlink(&outside, ws.join("link")).expect("symlink");
    assert!(!workspace_scoped("cat ./link/secret", &ws, &ws));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn sensitive_env_and_cd_and_find_exec_review() {
    for command in [
        "PATH=/evil ls",
        "LD_PRELOAD=/evil/x ls",
        "cd src && rm -rf /",
        "cd /tmp",
        "cd ..",
        "cd /work/other",
        "cd /work/proj/../../etc && ls",
        "find . -exec rm {} ;",
        "find . -delete",
        "ls --file=/etc/passwd",
        "ls --file=/etc/passwd extra",
    ] {
        assert!(!ok(command), "should review: {command}");
    }
}

#[test]
fn dev_null_and_in_repo_absolute_paths_are_fine() {
    assert!(ok("cargo build > /dev/null 2>&1"));
    assert!(ok("cargo test 2>&1"));
    assert!(ok("cargo build 2>/dev/null"));
    assert!(!ok("cat <<EOF"));
    assert!(!ok("cat <<-'EOF'
hi
EOF"));
    assert!(ok("ls /work/proj/src"));
    assert!(ok("rm -f /work/proj/target/x"));
    // A path that only looks inside because of `..` lexical folding is fine.
    assert!(ok("ls src/../src"));
    // `..` that escapes is not.
    assert!(!ok("ls ../../etc"));
}

#[test]
fn empty_and_assignments_only_review() {
    assert!(!ok(""));
    assert!(!ok("   "));
    assert!(!ok("FOO=bar"));
}

// The tokenizer must not be fooled by separators inside quotes.
#[test]
fn separators_inside_quotes_do_not_split() {
    let toks = tokens(r#"echo 'a ; b | c' "d e""#);
    assert_eq!(toks, vec!["echo", "a ; b | c", "d e"]);
}

#[test]
fn segments_split_on_shell_separators() {
    assert_eq!(segments("a && b | c ; d"), vec!["a ", " b ", " c ", " d"]);
    assert!(segments("   ").is_empty());
}
