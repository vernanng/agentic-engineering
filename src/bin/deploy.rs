//! Deploy binary: gate -> backup -> build -> swap -> health-check -> rollback.
//! Runs on the VPS (cd.yml) with CWD at the checkout. No shell scripts.
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const MANAGED_KEYS: [&str; 4] = [
    "LOOP_TOKEN",
    "OPENROUTER_API_BASE",
    "TURSO_DATABASE_URL",
    "TURSO_AUTH_TOKEN",
];

fn run(prog: &str, args: &[&str]) -> Result<(), String> {
    run_in(&std::env::current_dir().map_err(|e| format!("cwd: {e}"))?, prog, args)
}

fn run_in(dir: &Path, prog: &str, args: &[&str]) -> Result<(), String> {
    let st = Command::new(prog)
        .args(args)
        .current_dir(dir)
        .status()
        .map_err(|e| format!("spawn {prog}: {e}"))?;
    st.success()
        .then_some(())
        .ok_or_else(|| format!("{prog} {} failed: {st}", args.join(" ")))
}

fn is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
}

fn systemctl(action: &str) -> Result<(), String> {
    // VPS runner is unprivileged (sudoers covers these two); root needs no sudo.
    if is_root() {
        run("systemctl", &[action, "loop-app"])
    } else {
        run("sudo", &["-n", "systemctl", action, "loop-app"])
    }
}

// Merge GitHub-synced secrets (.env.cd) into .env: only the managed keys,
// only non-empty values; other .env keys untouched. Removes .env.cd.
fn merge_env(dir: &Path) -> Result<(), String> {
    let cd = dir.join(".env.cd");
    if !cd.exists() {
        return Ok(());
    }
    let incoming = std::fs::read_to_string(&cd).map_err(|e| format!("read .env.cd: {e}"))?;
    let mut got = std::collections::HashMap::new();
    for line in incoming.lines() {
        if let Some((k, v)) = line.split_once('=') {
            got.insert(k.to_string(), v.to_string());
        }
    }
    let env_path = dir.join(".env");
    let current = std::fs::read_to_string(&env_path).unwrap_or_default();
    let mut out: Vec<String> = current
        .lines()
        .filter(|l| {
            !MANAGED_KEYS
                .iter()
                .any(|k| l.split_once('=').map(|(k2, _)| k2 == *k).unwrap_or(false))
        })
        .map(|s| s.to_string())
        .collect();
    for k in MANAGED_KEYS {
        if let Some(v) = got.get(k) {
            if !v.is_empty() {
                out.push(format!("{k}={v}"));
            }
        }
    }
    let tmp = dir.join(".env.tmp");
    std::fs::write(&tmp, out.join("\n") + "\n").map_err(|e| format!("write .env: {e}"))?;
    #[cfg(unix)]
    std::fs::set_permissions(&tmp, std::os::unix::fs::PermissionsExt::from_mode(0o600))
        .map_err(|e| format!("chmod .env: {e}"))?;
    std::fs::rename(&tmp, &env_path).map_err(|e| format!("swap .env: {e}"))?;
    std::fs::remove_file(&cd).map_err(|e| format!("remove .env.cd: {e}"))?;
    Ok(())
}

fn env_file_token(root: &Path) -> String {
    std::fs::read_to_string(root.join(".env"))
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("LOOP_TOKEN="))
        .unwrap_or("")
        .to_string()
}

fn healthy(token: &str) -> bool {
    let mut req = ureq::get("http://localhost:8000/api/task-types");
    if !token.is_empty() {
        req = req.set("X-Loop-Token", token);
    }
    matches!(req.call(), Ok(r) if (200..300).contains(&r.status()))
}

fn restart(root: &Path, bin: &str) -> Result<(), String> {
    let link = root.join("releases/loop-app-latest");
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(bin, &link).map_err(|e| format!("link latest: {e}"))?;
    std::fs::write(root.join(".loop-app.current"), bin)
        .map_err(|e| format!("record current: {e}"))?;
    systemctl("restart")
}

fn log_tail(root: &Path) {
    let text = std::fs::read_to_string(root.join("loop-app.log")).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    for l in lines.iter().rev().take(20).rev() {
        eprintln!("{l}");
    }
}

fn install(root: &Path, name: &str, sha: &str) -> Result<Option<String>, String> {
    let src = PathBuf::from("target/release").join(name);
    if !src.exists() {
        return Ok(None);
    }
    // cp+mv: copying over the running binary fails (text busy).
    let tmp = root.join(format!("releases/.tmp-{name}-{sha}"));
    let dst = root.join(format!("releases/{name}-{sha}"));
    std::fs::copy(&src, &tmp).map_err(|e| format!("stage {name}: {e}"))?;
    std::fs::rename(&tmp, &dst).map_err(|e| format!("install {name}: {e}"))?;
    Ok(Some(dst.to_string_lossy().into_owned()))
}

fn deploy() -> Result<(), String> {
    // Runner services start with a minimal PATH; cargo lives under the user's home.
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/runner".into());
    let path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{home}/.cargo/bin:{path}"));
    // Source checkout is the CWD (cargo builds here); the live checkout that
    // serves the app is LOOP_CHECKOUT (db, releases, .env all live there).
    let root = std::env::var("LOOP_CHECKOUT")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir().map_err(|e| format!("cwd: {e}"))?);
    println!("==> sync main");
    run_in(&root, "git", &["fetch", "origin"])?;
    run_in(&root, "git", &["checkout", "main"])?;
    run_in(&root, "git", &["pull", "--ff-only", "origin", "main"])?;
    let sha_out = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .map_err(|e| format!("git sha: {e}"))?;
    if !sha_out.status.success() {
        return Err("git sha: rev-parse failed".into());
    }
    let sha = String::from_utf8_lossy(&sha_out.stdout).trim().to_string();
    // Sync GitHub-managed secrets, exactly the 4 managed keys.
    let mut cd = String::new();
    for k in MANAGED_KEYS {
        cd.push_str(&format!("{k}={}\n", std::env::var(k).unwrap_or_default()));
    }
    std::fs::write(root.join(".env.cd"), cd).map_err(|e| format!("write .env.cd: {e}"))?;
    merge_env(&root)?;
    // Secrets were only needed for .env.cd: scrub them so spawned children
    // (release gate, build) run hermetic, as when secrets arrived by pipe.
    for k in MANAGED_KEYS {
        std::env::remove_var(k);
    }

    println!("==> gate: cargo test");
    run("cargo", &["test", "--release", "--locked"])?;

    println!("==> backup db");
    std::fs::create_dir_all(root.join("backups")).map_err(|e| format!("mkdir backups: {e}"))?;
    std::fs::create_dir_all(root.join("releases")).map_err(|e| format!("mkdir releases: {e}"))?;
    if root.join("loop.db").exists() {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        std::fs::copy(
            root.join("loop.db"),
            root.join(format!("backups/loop-{secs}.db")),
        )
        .map_err(|e| format!("backup db: {e}"))?;
    }

    println!("==> build loop-app:{sha}");
    run("cargo", &["build", "--release"])?;
    let bin = install(&root, "loop-app", &sha)?.ok_or("target/release/loop-app missing")?;
    let _ = install(&root, "client", &sha)?; // optional, like before

    let prev = std::fs::read_to_string(root.join(".loop-app.current"))
        .unwrap_or_default()
        .trim()
        .to_string();

    println!("==> deploy loop-app:{sha}");
    restart(&root, &bin)?;

    println!("==> health check http://localhost:8000");
    let token = env_file_token(&root);
    for _ in 0..20 {
        if healthy(&token) {
            println!("deploy: OK loop-app:{sha} at http://localhost:8000");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    eprintln!("deploy: health check failed, log tail:");
    log_tail(&root);
    if !prev.is_empty() && Path::new(&prev).is_file() {
        eprintln!("deploy: rolling back to {prev}");
        restart(&root, &prev)?;
    } else {
        let _ = systemctl("stop");
    }
    Err(format!("deploy: health check failed for loop-app:{sha}"))
}

fn main() {
    if let Err(e) = deploy() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("deploy-test-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn merge_replaces_appends_and_skips_empties() {
        let d = tmpdir("merge");
        std::fs::write(
            d.join(".env"),
            "OPENROUTER_API_KEY=keep-me\nLOOP_TOKEN=old-token\nHOST=0.0.0.0\n",
        )
        .unwrap();
        std::fs::write(
            d.join(".env.cd"),
            "LOOP_TOKEN=new-token\nOPENROUTER_API_BASE=https://openrouter.ai\nTURSO_DATABASE_URL=\nTURSO_AUTH_TOKEN=tok123\n",
        )
        .unwrap();
        merge_env(&d).unwrap();
        let out = std::fs::read_to_string(d.join(".env")).unwrap();
        assert!(out.contains("LOOP_TOKEN=new-token\n"), "{out}");
        assert!(out.contains("OPENROUTER_API_BASE=https://openrouter.ai\n"), "{out}");
        assert!(out.contains("TURSO_AUTH_TOKEN=tok123\n"), "{out}");
        assert!(out.contains("OPENROUTER_API_KEY=keep-me\n"), "{out}");
        assert!(out.contains("HOST=0.0.0.0\n"), "{out}");
        assert!(!out.contains("TURSO_DATABASE_URL="), "empty value skipped, not appended: {out}");
        assert!(!d.join(".env.cd").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn merge_without_cd_leaves_env_alone() {
        let d = tmpdir("nocd");
        std::fs::write(d.join(".env"), "A=1\n").unwrap();
        merge_env(&d).unwrap();
        assert_eq!(std::fs::read_to_string(d.join(".env")).unwrap(), "A=1\n");
        let _ = std::fs::remove_dir_all(&d);
    }
}
