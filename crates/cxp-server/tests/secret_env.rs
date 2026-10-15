//! The serving process sheds the migration owner's credential (#27): the
//! real binary, started with `CXP_MIGRATION_DATABASE_URL` in its
//! environment, re-executes itself without it, so neither its environment
//! block (`/proc/<pid>/environ`, which `unsetenv` would not clear) nor its
//! log holds it. Linux only: it reads `/proc`.

#![cfg(target_os = "linux")]

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const SECRET: &str = "owner-password-must-not-survive";

#[test]
fn a_serving_process_reexecutes_without_the_migration_secret() {
    // A database that accepts the connection and never answers, so boot sits
    // in its connect (bounded by the pool's acquire timeout) while the test
    // looks at the process.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let db = silent.local_addr().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_cxp-server"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("CXP_PUBLIC_URL", "http://localhost:8080")
        .env("CXP_PLATFORM_URL", "http://localhost:8090")
        .env("CXP_INTROSPECTION_SECRET", "otto_rs_test")
        .env("CXP_PLATFORM_WEBHOOK_SECRET", "otto_whsec_test")
        .env("CXP_BIND", "127.0.0.1:0")
        .env("CXP_LOG_FORMAT", "text")
        .env("RUST_LOG", "info")
        .env("DATABASE_URL", format!("postgres://srv:srv@{db}/x"))
        .env(
            "CXP_MIGRATION_DATABASE_URL",
            format!("postgres://owner:{SECRET}@{db}/x"),
        )
        // Run from a directory with no .env.
        .current_dir(std::env::temp_dir())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let environ = format!("/proc/{}/environ", child.id());

    // Same pid before and after exec; the marker says the new image is in.
    let deadline = Instant::now() + Duration::from_secs(4);
    let after = loop {
        let env = std::fs::read(&environ).unwrap_or_default();
        if env
            .split(|b| *b == 0)
            .any(|kv| kv == b"CXP_MIGRATION_SECRET_REMOVED=1")
        {
            break env;
        }
        assert!(
            Instant::now() < deadline,
            "the serving process did not re-execute itself"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let text = String::from_utf8_lossy(&after);
    assert!(
        !text.contains(SECRET),
        "the owner's credential is still in the environment"
    );
    assert!(!text.contains("CXP_MIGRATION_DATABASE_URL"));
    assert!(
        text.contains("DATABASE_URL=postgres://srv:srv@"),
        "the rest is kept"
    );

    let _ = child.kill();
    let _ = child.wait();
    let mut out = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    assert!(!out.contains(SECRET), "logged: {out}");
    assert!(
        out.contains("removed CXP_MIGRATION_DATABASE_URL from the serving process's environment"),
        "{out}"
    );
    drop(silent);
}

/// `migrate` keeps it: it is the one command that needs it. With no database
/// to reach it fails, naming nothing secret.
#[test]
fn migrate_keeps_the_secret_and_never_logs_it() {
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let db = silent.local_addr().unwrap();
    drop(silent); // nothing listens: the connect fails at once
    let out = Command::new(env!("CARGO_BIN_EXE_cxp-server"))
        .arg("migrate")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("CXP_PUBLIC_URL", "https://cxp.example.com")
        .env("CXP_PLATFORM_URL", "https://platform.example.com")
        .env("CXP_INTROSPECTION_SECRET", "otto_rs_test")
        .env("CXP_PLATFORM_WEBHOOK_SECRET", "otto_whsec_test")
        .env("CXP_LOG_FORMAT", "text")
        .env("DATABASE_URL", format!("postgres://srv:srv@{db}/x"))
        .env(
            "CXP_MIGRATION_DATABASE_URL",
            format!("postgres://owner:{SECRET}@{db}/x"),
        )
        .current_dir(std::env::temp_dir())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!text.contains(SECRET), "{text}");
    assert!(!text.contains("CXP_MIGRATION_SECRET_REMOVED"), "{text}");
}
