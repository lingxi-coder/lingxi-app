//! Native diagnostic for a pending blocking `read_exact` on a TCP lane.
//! Each scenario is a child process so a stuck read remains bounded without
//! changing the socket with a receive timeout.
use std::{
    io::{self, Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    process::{Child, Command},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

fn scenario(partial: bool, local_first: bool) -> Result<(), String> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    let mut peer = TcpStream::connect(listener.local_addr().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let (mut reader, _) = listener.accept().map_err(|e| e.to_string())?;
    let control = reader.try_clone().map_err(|e| e.to_string())?;
    let (entered_tx, entered_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let read_thread = thread::spawn(move || {
        let mut header = [0u8; 8];
        entered_tx.send(()).unwrap();
        let result = reader.read_exact(&mut header);
        let description = result.map_err(|e| (e.kind(), e.raw_os_error()));
        let _ = done_tx.send(description);
    });
    if partial {
        peer.write_all(&[1, 2, 3]).map_err(|e| e.to_string())?;
    }
    entered_rx
        .recv_timeout(Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    // This delay improves the likelihood that the read is already pending. The
    // observed early/late outcome is evidence, not a scheduler guarantee.
    thread::sleep(Duration::from_millis(250));
    let name = format!("partial={partial} local_shutdown_first={local_first}");
    if local_first {
        let result = control.shutdown(Shutdown::Both);
        println!(
            "{name}: local shutdown={:?}",
            result.as_ref().err().map(|e| (e.kind(), e.raw_os_error()))
        );
        result.map_err(|e| e.to_string())?;
    }
    let start = Instant::now();
    let early = done_rx.recv_timeout(Duration::from_secs(1));
    println!(
        "{name}: read before peer FIN={early:?} after {:?}",
        start.elapsed()
    );
    if matches!(early, Err(mpsc::RecvTimeoutError::Disconnected)) {
        return Err(format!("{name}: read thread stopped without a result"));
    }
    if local_first {
        if matches!(early, Err(mpsc::RecvTimeoutError::Timeout)) {
            let result = peer.shutdown(Shutdown::Write);
            println!(
                "{name}: peer FIN result={:?}",
                result.as_ref().err().map(|e| (e.kind(), e.raw_os_error()))
            );
            let after_fin = done_rx.recv_timeout(Duration::from_secs(3));
            println!("{name}: read after peer FIN={after_fin:?}");
            match after_fin {
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    println!("{name}: pending read observed; terminating isolated scenario");
                    return Ok(());
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(format!("{name}: read thread stopped without a result"));
                }
                Ok(_) => {}
            }
        }
    } else {
        if early.is_ok() {
            return Err(format!("{name}: read returned before peer FIN"));
        }
        peer.shutdown(Shutdown::Write).map_err(|e| e.to_string())?;
        let after_fin = done_rx.recv_timeout(Duration::from_secs(3));
        println!("{name}: read after peer FIN={after_fin:?}");
        if !matches!(after_fin, Ok(Err((std::io::ErrorKind::UnexpectedEof, _)))) {
            return Err(format!("{name}: peer FIN did not end read_exact with EOF"));
        }
    }
    read_thread
        .join()
        .map_err(|_| format!("{name}: read thread panicked"))?;
    Ok(())
}

// Poll only nonblocking process status. A failed kill or inconclusive status
// never becomes an exit receipt and never introduces an unbounded wait.
fn poll_reap<T>(
    limit: Duration,
    mut try_wait: impl FnMut() -> io::Result<Option<T>>,
) -> Result<T, String> {
    let deadline = Instant::now() + limit;
    let mut last_error = None;
    loop {
        match try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => last_error = Some(error.to_string()),
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "termination unconfirmed after {limit:?}; last wait error={last_error:?}"
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn terminate_with_receipt(child: &mut Child, reason: &str, limit: Duration) -> String {
    let kill_error = child.kill().err().map(|e| e.to_string());
    match poll_reap(limit, || child.try_wait()) {
        Ok(status) => format!("{reason}; child reaped: {status}; kill error={kill_error:?}"),
        Err(error) => format!("{reason}; {error}; kill error={kill_error:?}"),
    }
}

fn supervise(child: &mut Child, limit: Duration) -> Result<(), String> {
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(format!("socket diagnostic subprocess failed: {status}"));
            }
            Ok(None) => {}
            Err(error) => {
                return Err(terminate_with_receipt(
                    child,
                    &format!("socket diagnostic status failed: {error}"),
                    Duration::from_secs(2),
                ));
            }
        }
        if Instant::now() >= deadline {
            return Err(terminate_with_receipt(
                child,
                "socket diagnostic subprocess exceeded deadline",
                Duration::from_secs(2),
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn verify_termination_controls() -> Result<(), String> {
    // Exercise the actual timeout and OS process-kill path, with an exit
    // witness, before probing sockets on this runner.
    let mut child = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .arg("--hang-fixture")
        .spawn()
        .map_err(|e| e.to_string())?;
    let outcome = supervise(&mut child, Duration::from_millis(250));
    let Err(message) = outcome else {
        return Err("blocked child was incorrectly reported successful".into());
    };
    if !message.contains("exceeded deadline") || !message.contains("child reaped:") {
        return Err(format!(
            "timeout control did not prove child exit: {message}"
        ));
    }
    println!("timeout-and-reap control: {message}");

    // Controlled non-completion and status-error paths must both fail closed.
    for fail_status in [false, true] {
        let result = poll_reap(Duration::from_millis(60), || -> io::Result<Option<()>> {
            if fail_status {
                Err(io::Error::other("synthetic status failure"))
            } else {
                Ok(None)
            }
        });
        let Err(message) = result else {
            return Err("missing child exit was incorrectly confirmed".into());
        };
        if !message.contains("termination unconfirmed") {
            return Err(format!("unexpected missing-exit result: {message}"));
        }
        println!("unconfirmed-reap control: status_error={fail_status}; {message}");
    }
    Ok(())
}

fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() == 2 && args[1] == "--hang-fixture" {
        loop {
            thread::sleep(Duration::from_secs(60));
        }
    }
    if args.len() == 2 && args[1] == "--controls" {
        return verify_termination_controls();
    }
    if args.len() == 3 {
        let partial = match args[1].as_str() {
            "idle" => false,
            "partial" => true,
            _ => return Err("invalid read scenario".into()),
        };
        let local_first = match args[2].as_str() {
            "peer" => false,
            "local" => true,
            _ => return Err("invalid shutdown scenario".into()),
        };
        return scenario(partial, local_first);
    }
    if args.len() != 1 {
        return Err("invalid scenario arguments".into());
    }
    for partial in [false, true] {
        for local in [false, true] {
            let mut child = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
                .arg(if partial { "partial" } else { "idle" })
                .arg(if local { "local" } else { "peer" })
                .spawn()
                .map_err(|e| e.to_string())?;
            supervise(&mut child, Duration::from_secs(8))?;
        }
    }
    Ok(())
}
