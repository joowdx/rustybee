use rustybee::config::Config;
use rustybee::store;
use std::io::{BufRead, IsTerminal, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::process::ExitCode;
use std::time::Duration;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

const USAGE: &str =
    "usage: rustybee [serve | set-password [--stdin] | backup <path> | healthcheck | version]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("serve");
    let result = match cmd {
        "serve" => serve(),
        "set-password" => set_password(args.iter().any(|a| a == "--stdin")),
        "backup" => match args.get(1) {
            Some(p) => backup(p),
            None => Err(USAGE.to_string()),
        },
        "healthcheck" => healthcheck(),
        "version" | "--version" | "-V" => {
            println!("rustybee {}", rustybee::app::VERSION);
            Ok(())
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rustybee: {e}");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(std::io::stderr().is_terminal())
        .with_writer(std::io::stderr)
        .init();
}

fn serve() -> Result<(), String> {
    init_tracing();
    let cfg = Config::from_env()?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async {
        let running = rustybee::app::start(cfg).await?;
        running.wait_for_signal().await;
        Ok(())
    })
}

fn read_password(stdin_mode: bool) -> Result<String, String> {
    let stdin = std::io::stdin();
    if stdin_mode || !stdin.is_terminal() {
        let mut line = String::new();
        stdin
            .lock()
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        return Ok(line.trim_end_matches(['\r', '\n']).to_string());
    }
    // Interactive: turn echo off while typing (best effort).
    let stty = |arg: &str| {
        let _ = std::process::Command::new("stty")
            .arg(arg)
            .stdin(std::process::Stdio::inherit())
            .status();
    };
    let ask = |prompt: &str| -> Result<String, String> {
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
        stty("-echo");
        let mut line = String::new();
        let r = stdin.lock().read_line(&mut line);
        stty("echo");
        eprintln!();
        r.map_err(|e| e.to_string())?;
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    };
    let a = ask("New admin password: ")?;
    let b = ask("Repeat: ")?;
    if a != b {
        return Err("passwords do not match".into());
    }
    Ok(a)
}

fn set_password(stdin_mode: bool) -> Result<(), String> {
    let cfg = Config::from_env()?;
    let pw = read_password(stdin_mode)?;
    if pw.chars().count() < store::auth::MIN_PASSWORD {
        return Err(format!(
            "the password needs at least {} characters",
            store::auth::MIN_PASSWORD
        ));
    }
    std::fs::create_dir_all(&cfg.data_dir).map_err(|e| e.to_string())?;
    let conn = store::open_rw(&cfg.db_path()).map_err(|e| e.to_string())?;
    store::auth::set_password_hash(&conn, &store::auth::hash_password(&pw))
        .map_err(|e| e.to_string())?;
    // A password change invalidates every existing session.
    conn.execute("DELETE FROM sessions", [])
        .map_err(|e| e.to_string())?;
    eprintln!("admin password updated");
    Ok(())
}

fn backup(dest: &str) -> Result<(), String> {
    let cfg = Config::from_env()?;
    let conn = store::open_rw(&cfg.db_path()).map_err(|e| e.to_string())?;
    conn.execute("VACUUM INTO ?1", [dest])
        .map_err(|e| e.to_string())?;
    eprintln!("backup written to {dest}");
    Ok(())
}

/// GET /api/v1/health over plain TCP; exit 0 on HTTP 200.
fn healthcheck() -> Result<(), String> {
    let bind = std::env::var("RUSTYBEE_BIND").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let mut addr: SocketAddr = bind
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or("no address")?;
    if addr.ip().is_unspecified() {
        addr.set_ip(std::net::Ipv4Addr::LOCALHOST.into());
    }
    let timeout = Duration::from_secs(3);
    let mut s = TcpStream::connect_timeout(&addr, timeout).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    s.set_write_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    s.write_all(b"GET /api/v1/health HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .map_err(|e| e.to_string())?;
    let mut buf = [0u8; 64];
    let n = s.read(&mut buf).map_err(|e| e.to_string())?;
    let head = String::from_utf8_lossy(&buf[..n]);
    if head.starts_with("HTTP/1.1 200") || head.starts_with("HTTP/1.0 200") {
        Ok(())
    } else {
        Err(format!("unhealthy: {}", head.lines().next().unwrap_or("")))
    }
}
